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
    capture::{PipewireTarget, VideoConfig, add_video_branch},
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
}

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
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub encoder: Encoder,
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

        let window = gst::ClockTime::from_seconds(cfg.replay_seconds.into());
        let buffer = Arc::new(Mutex::new(ReplayBuffer::new(window, cfg.audio.len())));
        let pipeline = gst::Pipeline::new();

        let parse = add_video_branch(&pipeline, source.target(), encoder, &cfg.video)?;
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

        pipeline
            .set_state(gst::State::Playing)
            .context("starting capture pipeline")?;
        tracing::info!(
            ?encoder,
            replay_seconds = cfg.replay_seconds,
            "recorder started"
        );

        Ok(Self {
            pipeline,
            buffer,
            encoder,
            replay_seconds: cfg.replay_seconds,
            error,
            stop,
            bus_thread: Some(bus_thread),
            source: Some(source),
        })
    }

    pub fn status(&self) -> Status {
        Status {
            encoder: self.encoder,
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
