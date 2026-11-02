//! Always-on recorder: capture → encode → replay buffer, with save on demand.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
};

use anyhow::{Context, Result};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use serde::{Deserialize, Serialize};

use crate::{
    audio::{AudioSource, add_audio_branch},
    capture::{CapturePath, PipewireTarget, VideoConfig, add_video_branch},
    encoder::Encoder,
    kwin::KwinStream,
    portal::ScreenSource,
    ring::{ReplayBuffer, running_time_buffer},
    save::write_mp4,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecorderConfig {
    pub video: VideoConfig,
    /// `None` probes for the best working encoder.
    pub encoder: Option<Encoder>,
    pub replay_seconds: u32,
    pub audio: Vec<AudioSource>,
    /// AAC bitrate per audio track, bits/s.
    #[serde(default = "default_audio_bitrate")]
    pub audio_bitrate: i32,
    /// Draw the mouse cursor into the video.
    #[serde(default = "yes")]
    pub cursor: bool,
    /// Try the zero-copy (DMA-BUF) capture path before the copy path.
    #[serde(default = "yes")]
    pub zero_copy: bool,
}

/// How long the zero-copy pipeline gets to deliver a first frame before it
/// is judged not to work on this machine.
const ZERO_COPY_PROBE: std::time::Duration = std::time::Duration::from_secs(4);

fn default_audio_bitrate() -> i32 {
    160_000
}

fn yes() -> bool {
    true
}

impl Default for RecorderConfig {
    fn default() -> Self {
        Self {
            video: VideoConfig::default(),
            encoder: None,
            replay_seconds: 30,
            audio: vec![AudioSource::Desktop],
            audio_bitrate: default_audio_bitrate(),
            cursor: true,
            zero_copy: true,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub encoder: Encoder,
    /// Which capture path is in use.
    pub path: CapturePath,
    pub buffered_seconds: f64,
    pub replay_seconds: u32,
    pub error: Option<String>,
}

/// What is being captured. Keeps the underlying session alive.
pub enum CaptureSource {
    /// A screen or window picked through the xdg portal.
    Portal(ScreenSource),
    /// A window streamed by KWin directly (no picker).
    Kwin(KwinStream),
}

impl CaptureSource {
    fn target(&self) -> PipewireTarget {
        match self {
            CaptureSource::Portal(s) => s.target(),
            CaptureSource::Kwin(s) => PipewireTarget::Serial(s.pipewire_serial()),
        }
    }

    async fn close(self) -> Result<()> {
        match self {
            CaptureSource::Portal(s) => s.close().await,
            CaptureSource::Kwin(_) => Ok(()),
        }
    }
}

pub struct Recorder {
    pipeline: gst::Pipeline,
    buffer: Arc<Mutex<ReplayBuffer>>,
    encoder: Encoder,
    path: CapturePath,
    replay_seconds: u32,
    error: Arc<Mutex<Option<String>>>,
    stop: Arc<AtomicBool>,
    bus_thread: Option<JoinHandle<()>>,
    // Dropped last: pipewiresrc borrows its fd.
    source: Option<CaptureSource>,
}

impl Recorder {
    /// Start buffering `source`.
    pub async fn start(cfg: &RecorderConfig, source: CaptureSource) -> Result<Self> {
        let encoder = match cfg.encoder {
            Some(e) => e,
            None => {
                let codec = cfg.video.codec;
                tokio::task::spawn_blocking(move || Encoder::probe(codec))
                    .await?
                    .with_context(|| format!("no working {codec:?} encoder found"))?
            }
        };

        let mut candidates = Vec::new();
        if cfg.zero_copy && encoder.gpu_memory().is_some() {
            candidates.push(CapturePath::ZeroCopy);
        }
        candidates.push(CapturePath::Copy);

        let target = source.target();
        let mut last_err = None;
        for (i, path) in candidates.iter().copied().enumerate() {
            let last = i + 1 == candidates.len();
            let built = match Pipeline::build(cfg, encoder, target, path) {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!(?path, "can't build capture pipeline: {e:#}");
                    last_err = Some(e);
                    continue;
                }
            };
            if let Err(e) = built
                .pipeline
                .set_state(gst::State::Playing)
                .context("starting capture pipeline")
            {
                built.teardown().await;
                last_err = Some(e);
                continue;
            }
            // The copy path is the known-good one; only the zero-copy path
            // has to prove itself with a first frame.
            if !last && !built.probe(ZERO_COPY_PROBE).await {
                let why = built
                    .error
                    .lock()
                    .unwrap()
                    .clone()
                    .unwrap_or_else(|| "no frames arrived".into());
                tracing::warn!(?path, "zero-copy capture failed, falling back: {why}");
                built.teardown().await;
                last_err = Some(anyhow::anyhow!(why));
                continue;
            }
            tracing::info!(
                ?encoder,
                ?path,
                replay_seconds = cfg.replay_seconds,
                "recorder started"
            );
            return Ok(Self {
                pipeline: built.pipeline,
                buffer: built.buffer,
                encoder,
                path,
                replay_seconds: cfg.replay_seconds,
                error: built.error,
                stop: built.stop,
                bus_thread: Some(built.bus_thread),
                source: Some(source),
            });
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("no capture path available")))
    }

    pub fn status(&self) -> Status {
        Status {
            encoder: self.encoder,
            path: self.path,
            buffered_seconds: self.buffer.lock().unwrap().buffered().seconds_f64(),
            replay_seconds: self.replay_seconds,
            error: self.error.lock().unwrap().clone(),
        }
    }

    /// Token to persist so the next start skips the screen picker.
    pub fn restore_token(&self) -> Option<&str> {
        match self.source.as_ref()? {
            CaptureSource::Portal(s) => s.restore_token(),
            CaptureSource::Kwin(_) => None,
        }
    }

    /// Save the last `seconds` (default: the whole buffer) to `path`.
    pub async fn save(&self, seconds: Option<u32>, path: &Path) -> Result<PathBuf> {
        let duration = seconds
            .map(|s| gst::ClockTime::from_seconds(s.into()))
            .unwrap_or_else(|| self.buffer.lock().unwrap().window());
        let clip = self.buffer.lock().unwrap().snapshot(duration)?;
        let path = path.to_owned();
        tokio::task::spawn_blocking(move || {
            write_mp4(&clip, &path)?;
            tracing::info!(path = %path.display(), duration = %clip.duration, "saved clip");
            Ok(path)
        })
        .await?
    }

    pub async fn stop(mut self) -> Result<()> {
        if let Some(rate) = self.pipeline.by_name("rate") {
            tracing::info!(
                frames_in = rate.property::<u64>("in"),
                out = rate.property::<u64>("out"),
                duplicated = rate.property::<u64>("duplicate"),
                dropped = rate.property::<u64>("drop"),
                clock = ?self.pipeline.clock().map(|c| c.name()),
                "video pacing"
            );
        }
        self.pipeline.set_state(gst::State::Null)?;
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.bus_thread.take() {
            let _ = tokio::task::spawn_blocking(move || t.join()).await;
        }
        match self.source.take() {
            Some(s) => s.close().await,
            None => Ok(()),
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// A built capture pipeline that hasn't proven itself yet.
struct Pipeline {
    pipeline: gst::Pipeline,
    buffer: Arc<Mutex<ReplayBuffer>>,
    error: Arc<Mutex<Option<String>>>,
    stop: Arc<AtomicBool>,
    bus_thread: JoinHandle<()>,
}

impl Pipeline {
    fn build(
        cfg: &RecorderConfig,
        encoder: Encoder,
        target: PipewireTarget,
        path: CapturePath,
    ) -> Result<Self> {
        let window = gst::ClockTime::from_seconds(cfg.replay_seconds.into());
        let buffer = Arc::new(Mutex::new(ReplayBuffer::new(window, cfg.audio.len())));
        let pipeline = gst::Pipeline::new();

        let parse = add_video_branch(&pipeline, target, encoder, &cfg.video, path)?;
        let buf = buffer.clone();
        link_appsink(&pipeline, &parse, "video-sink", move |caps, buffer| {
            let mut b = buf.lock().unwrap();
            if let Some(caps) = caps {
                b.video.set_caps(caps);
            }
            b.push_video(buffer);
        })?;

        for (i, src) in cfg.audio.iter().enumerate() {
            let parse = add_audio_branch(&pipeline, src, cfg.audio_bitrate)?;
            let buf = buffer.clone();
            link_appsink(
                &pipeline,
                &parse,
                &format!("{}-sink", src.name()),
                move |caps, buffer| {
                    let mut b = buf.lock().unwrap();
                    if let Some(caps) = caps {
                        b.audio[i].set_caps(caps);
                    }
                    b.push_audio(i, buffer);
                },
            )?;
        }

        let error = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let bus_thread = spawn_bus_watch(&pipeline, error.clone(), stop.clone())?;
        Ok(Self {
            pipeline,
            buffer,
            error,
            stop,
            bus_thread,
        })
    }

    /// True once a video frame has been buffered without an error; false if
    /// an error is posted or nothing arrives within `timeout`.
    async fn probe(&self, timeout: std::time::Duration) -> bool {
        let started = std::time::Instant::now();
        while started.elapsed() < timeout {
            if self.error.lock().unwrap().is_some() {
                return false;
            }
            if self.buffer.lock().unwrap().video.oldest_pts().is_some() {
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        false
    }

    async fn teardown(self) {
        let _ = self.pipeline.set_state(gst::State::Null);
        self.stop.store(true, Ordering::Relaxed);
        let _ = tokio::task::spawn_blocking(move || self.bus_thread.join()).await;
    }
}

/// Terminate `tail` in an appsink that hands each buffer to `on_buffer`,
/// along with the caps whenever they change.
fn link_appsink(
    pipeline: &gst::Pipeline,
    tail: &gst::Element,
    name: &str,
    mut on_buffer: impl FnMut(Option<gst::Caps>, gst::Buffer) + Send + 'static,
) -> Result<()> {
    let mut last_caps: Option<gst::Caps> = None;
    let sink = gst_app::AppSink::builder()
        .name(name)
        // Pull as fast as possible; the ring keeps capture timestamps.
        .sync(false)
        .callbacks(
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                    let Some(buffer) = running_time_buffer(&sample) else {
                        return Ok(gst::FlowSuccess::Ok);
                    };
                    let caps = sample
                        .caps_owned()
                        .filter(|c| last_caps.as_ref() != Some(c));
                    if let Some(c) = &caps {
                        last_caps = Some(c.clone());
                    }
                    on_buffer(caps, buffer);
                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        )
        .build();
    pipeline.add(&sink)?;
    tail.link(&sink)?;
    Ok(())
}

fn spawn_bus_watch(
    pipeline: &gst::Pipeline,
    error: Arc<Mutex<Option<String>>>,
    stop: Arc<AtomicBool>,
) -> Result<JoinHandle<()>> {
    let bus = pipeline.bus().context("pipeline has no bus")?;
    Ok(std::thread::Builder::new()
        .name("clipsh-bus".into())
        .spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let Some(msg) = bus.timed_pop(gst::ClockTime::from_mseconds(200)) else {
                    continue;
                };
                match msg.view() {
                    gst::MessageView::Error(e) => {
                        let text = format!(
                            "{} from {:?}: {:?}",
                            e.error(),
                            msg.src().map(|s| s.path_string()),
                            e.debug()
                        );
                        tracing::error!("{text}");
                        *error.lock().unwrap() = Some(text);
                    }
                    gst::MessageView::Warning(w) => {
                        tracing::warn!(
                            "{} from {:?}",
                            w.error(),
                            msg.src().map(|s| s.path_string())
                        );
                    }
                    _ => {}
                }
            }
        })?)
}
