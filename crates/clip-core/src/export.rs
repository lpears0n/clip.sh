//! Render an edited clip: trim to a range and mix the audio tracks at chosen
//! levels into one track (so the result plays the same everywhere),
//! optionally fitted under a file size (Discord's upload limits).

use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, bail};
use gstreamer as gst;
use gstreamer::prelude::*;
use serde::Serialize;

use crate::{
    capture::VideoConfig,
    encoder::{Codec, Encoder},
    media,
};

#[derive(Debug, Clone)]
pub struct Edit {
    pub start: gst::ClockTime,
    pub end: gst::ClockTime,
    /// Linear gain per source audio track, in file order (1.0 = unchanged).
    /// Missing entries default to 1.0.
    pub volumes: Vec<f64>,
    /// Fit the file under this many bytes: the bitrate comes from the
    /// length, and the picture is scaled down (and to 30 fps) when the
    /// budget is too small for the full size. `None` keeps full quality.
    pub max_bytes: Option<u64>,
}

const AAC_BITRATE: i32 = 192_000;

/// How a size-limited export is encoded.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct SizePlan {
    pub video_kbps: u32,
    pub audio_kbps: u32,
    /// Picture size class: a 16:9 picture this tall, or the same number of
    /// pixels at other shapes. Never scaled up.
    pub max_height: u32,
    /// Frame rate to drop to; `None` keeps the source's.
    pub fps: Option<u32>,
}

/// Share of the byte budget given to the streams; the rest covers the
/// container and encoder overshoot.
const SIZE_MARGIN: f64 = 0.92;
const MIN_VIDEO_KBPS: u32 = 200;
const MIN_AUDIO_KBPS: u32 = 64;
/// Bits per pixel per frame a size class needs to look clean; lower
/// rates step down to the next class.
const MIN_BPP: f64 = 0.05;
/// Size classes, best first: (height, fps to drop to).
const LADDER: [(u32, Option<u32>); 7] = [
    (2160, None),
    (1440, None),
    (1080, None),
    (720, None),
    (720, Some(30)),
    (480, Some(30)),
    (360, Some(30)),
];

/// Bitrates and picture size to fit `seconds` of video (with audio, if
/// `audio`) under `max_bytes`. Errors when it can't fit at a watchable
/// rate, saying how long a clip can be.
pub fn plan_size(max_bytes: u64, seconds: f64, audio: bool) -> Result<SizePlan> {
    if seconds <= 0.0 {
        bail!("nothing to render");
    }
    let total_kbps = (max_bytes as f64 * 8.0 * SIZE_MARGIN / seconds / 1000.0) as u32;
    let audio_kbps = match (audio, total_kbps) {
        (false, _) => 0,
        (true, t) if t >= 1500 => 128,
        (true, t) if t >= 600 => 96,
        _ => MIN_AUDIO_KBPS,
    };
    let video_kbps = total_kbps.saturating_sub(audio_kbps);
    if video_kbps < MIN_VIDEO_KBPS {
        let floor = MIN_VIDEO_KBPS + if audio { MIN_AUDIO_KBPS } else { 0 };
        let longest = max_bytes as f64 * 8.0 * SIZE_MARGIN / (floor as f64 * 1000.0);
        bail!(
            "too long to fit in {}: trim it to under {}",
            mb(max_bytes),
            minutes(longest)
        );
    }
    let (max_height, fps) = LADDER
        .iter()
        .copied()
        .find(|&(h, fps)| {
            let pixels = h as f64 * h as f64 * 16.0 / 9.0;
            video_kbps as f64 * 1000.0 >= pixels * fps.unwrap_or(60) as f64 * MIN_BPP
        })
        .unwrap_or(LADDER[LADDER.len() - 1]);
    Ok(SizePlan {
        video_kbps,
        audio_kbps,
        max_height,
        fps,
    })
}

fn mb(bytes: u64) -> String {
    if bytes < 1_000_000 {
        format!("{} kB", bytes / 1000)
    } else {
        format!("{} MB", (bytes as f64 / 1e6).round())
    }
}

fn minutes(seconds: f64) -> String {
    let s = seconds.floor() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// The picture size for a `src_w`×`src_h` source under `max_height`'s
/// pixel budget: the same shape, even sides, never larger than the source.
pub fn scaled_size(src_w: u32, src_h: u32, max_height: u32) -> (u32, u32) {
    let even = |v: f64| ((v / 2.0).round() as u32 * 2).max(2);
    let budget = max_height as f64 * max_height as f64 * 16.0 / 9.0;
    let pixels = src_w as f64 * src_h as f64;
    let factor = if pixels > budget {
        (budget / pixels).sqrt()
    } else {
        1.0
    };
    (even(src_w as f64 * factor), even(src_h as f64 * factor))
}

/// Progress reporting and cancellation for a render.
#[derive(Default)]
pub struct Control {
    /// Called with the fraction done (0..=1) a few times a second.
    pub on_progress: Option<Box<dyn Fn(f64) + Send>>,
    /// Set to stop; the partial output is removed and `render` returns an
    /// error whose message contains "cancelled".
    pub cancel: Option<Arc<AtomicBool>>,
}

/// Re-encode `input[start..end]` to `output` as H.264 (edits are for sharing,
/// so they favour compatibility). `encoder` must support H.264. Blocks until
/// done.
pub fn render(
    input: &Path,
    output: &Path,
    edit: &Edit,
    encoder: Encoder,
    cfg: &VideoConfig,
) -> Result<()> {
    render_with(input, output, edit, encoder, cfg, Control::default())
}

/// [`render`], reporting progress and honouring cancellation.
pub fn render_with(
    input: &Path,
    output: &Path,
    edit: &Edit,
    encoder: Encoder,
    cfg: &VideoConfig,
    control: Control,
) -> Result<()> {
    let cfg = VideoConfig {
        codec: Codec::H264,
        ..cfg.clone()
    };
    if edit.end <= edit.start {
        bail!("end must be after start");
    }
    let audio_tracks = media::inspect(input)?.audio_tracks;
    if let Some(dir) = output.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let Some(max_bytes) = edit.max_bytes else {
        return render_once(
            input,
            output,
            edit,
            encoder,
            &cfg,
            audio_tracks,
            None,
            &control,
        );
    };
    let seconds = (edit.end - edit.start).seconds_f64();
    let mut plan = plan_size(max_bytes, seconds, audio_tracks > 0)?;
    // One pass with a rate from the length usually lands under; encoders
    // can overshoot on busy footage, so a file that came out too big is
    // redone at a rate scaled down by how much it missed.
    for _ in 0..3 {
        let cfg = VideoConfig {
            bitrate_kbps: plan.video_kbps,
            fps: plan.fps.map_or(cfg.fps, |f| f.min(cfg.fps)),
            ..cfg.clone()
        };
        render_once(
            input,
            output,
            edit,
            encoder,
            &cfg,
            audio_tracks,
            Some(plan),
            &control,
        )?;
        let size = std::fs::metadata(output)?.len();
        if size <= max_bytes {
            tracing::info!(size, max_bytes, ?plan, "fitted export");
            return Ok(());
        }
        let scaled = plan.video_kbps as f64 * max_bytes as f64 / size as f64 * 0.95;
        tracing::info!(
            size,
            max_bytes,
            ?plan,
            "export too big; redoing at {scaled:.0} kb/s"
        );
        if scaled < MIN_VIDEO_KBPS as f64 {
            break;
        }
        plan.video_kbps = scaled as u32;
    }
    let _ = std::fs::remove_file(output);
    bail!(
        "couldn't fit the clip in {}; trim it shorter",
        mb(max_bytes)
    )
}

/// One render; `plan` scales the picture and sets the audio rate.
#[allow(clippy::too_many_arguments)]
fn render_once(
    input: &Path,
    output: &Path,
    edit: &Edit,
    encoder: Encoder,
    cfg: &VideoConfig,
    audio_tracks: usize,
    plan: Option<SizePlan>,
    control: &Control,
) -> Result<()> {
    let pipeline = gst::Pipeline::new();
    let dec = gst::ElementFactory::make("uridecodebin")
        .property("uri", gst::glib::filename_to_uri(input, None)?)
        .build()?;
    let mux = gst::ElementFactory::make("mp4mux")
        .property("faststart", true)
        .build()?;
    let sink = gst::ElementFactory::make("filesink")
        .property("location", output)
        .build()?;
    pipeline.add_many([&dec, &mux, &sink])?;
    mux.link(&sink)?;

    // Video: decoded frames → (scaled to the plan) → encoder → mux.
    let vqueue = gst::ElementFactory::make("queue").build()?;
    let mut vchain = vec![
        vqueue.clone(),
        gst::ElementFactory::make("videoconvert").build()?,
    ];
    // The size is set once the decoder says how big the source is.
    let scale_caps = match plan {
        Some(plan) => {
            let caps = gst::ElementFactory::make("capsfilter").build()?;
            vchain.push(gst::ElementFactory::make("videoscale").build()?);
            if plan.fps.is_some() {
                vchain.push(gst::ElementFactory::make("videorate").build()?);
            }
            vchain.push(caps.clone());
            Some((caps, plan))
        }
        None => None,
    };
    vchain.extend(encoder.upload_chain()?);
    vchain.push(encoder.build(cfg)?);
    vchain.push(
        gst::ElementFactory::make("capsfilter")
            .property("caps", cfg.codec.caps())
            .build()?,
    );
    vchain.push(cfg.codec.make_parser()?);
    pipeline.add_many(&vchain)?;
    gst::Element::link_many(&vchain)?;
    vchain.last().unwrap().link(&mux)?;

    // Audio: every source track → volume → one mixer → AAC → mux. Only built
    // when there's audio; a mixer with no inputs would never finish.
    let mixer = if audio_tracks > 0 {
        let mixer = gst::ElementFactory::make("audiomixer").build()?;
        let chain = [
            mixer.clone(),
            gst::ElementFactory::make("audioconvert").build()?,
            gst::ElementFactory::make("audioresample").build()?,
            gst::ElementFactory::make("capsfilter")
                .property(
                    "caps",
                    gst::Caps::builder("audio/x-raw")
                        .field("rate", 48_000i32)
                        .field("channels", 2i32)
                        .build(),
                )
                .build()?,
            aac_encoder(plan.map_or(AAC_BITRATE, |p| p.audio_kbps as i32 * 1000))?,
            gst::ElementFactory::make("aacparse").build()?,
        ];
        pipeline.add_many(&chain)?;
        gst::Element::link_many(&chain)?;
        chain[5].link(&mux)?;
        Some(mixer)
    } else {
        None
    };

    let link_error: Arc<Mutex<Option<String>>> = Arc::default();
    {
        let pipeline = pipeline.downgrade();
        let volumes = edit.volumes.clone();
        let link_error = link_error.clone();
        let next_audio = Mutex::new(0usize);
        dec.connect_pad_added(move |_, pad| {
            let Some(pipeline) = pipeline.upgrade() else {
                return;
            };
            let caps = pad.current_caps().unwrap_or_else(|| pad.query_caps(None));
            let Some(name) = caps.structure(0).map(|s| s.name().to_string()) else {
                return;
            };
            let result = (|| -> Result<()> {
                if name.starts_with("video/") {
                    if let Some((filter, plan)) = &scale_caps {
                        let s = caps.structure(0).context("video caps")?;
                        let (w, h) = (s.get::<i32>("width")?, s.get::<i32>("height")?);
                        let (w, h) = scaled_size(w as u32, h as u32, plan.max_height);
                        let mut out = gst::Caps::builder("video/x-raw")
                            .field("width", w as i32)
                            .field("height", h as i32)
                            .field("pixel-aspect-ratio", gst::Fraction::new(1, 1));
                        if let Some(fps) = plan.fps {
                            out = out.field("framerate", gst::Fraction::new(fps as i32, 1));
                        }
                        filter.set_property("caps", out.build());
                    }
                    pad.link(&vqueue.static_pad("sink").unwrap())?;
                } else if name.starts_with("audio/")
                    && let Some(mixer) = &mixer
                {
                    let i = {
                        let mut n = next_audio.lock().unwrap();
                        *n += 1;
                        *n - 1
                    };
                    let gain = volumes.get(i).copied().unwrap_or(1.0).clamp(0.0, 4.0);
                    let chain = [
                        gst::ElementFactory::make("queue").build()?,
                        gst::ElementFactory::make("audioconvert").build()?,
                        gst::ElementFactory::make("audioresample").build()?,
                        gst::ElementFactory::make("volume")
                            .property("volume", gain)
                            .build()?,
                    ];
                    pipeline.add_many(&chain)?;
                    gst::Element::link_many(&chain)?;
                    chain[3].link(mixer)?;
                    for e in &chain {
                        e.sync_state_with_parent()?;
                    }
                    pad.link(&chain[0].static_pad("sink").unwrap())?;
                }
                Ok(())
            })();
            if let Err(e) = result {
                *link_error.lock().unwrap() = Some(format!("{e:#}"));
            }
        });
    }

    let result = (|| -> Result<()> {
        pipeline.set_state(gst::State::Paused)?;
        wait(&pipeline, gst::MessageType::AsyncDone)?;
        pipeline.seek(
            1.0,
            gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
            gst::SeekType::Set,
            edit.start,
            gst::SeekType::Set,
            edit.end,
        )?;
        wait(&pipeline, gst::MessageType::AsyncDone)?;
        pipeline.set_state(gst::State::Playing)?;
        run(&pipeline, edit, control)
    })();
    let _ = pipeline.set_state(gst::State::Null);
    let result = match link_error.lock().unwrap().take() {
        Some(e) => result.and(Err(anyhow::anyhow!(e))),
        None => result,
    };
    if result.is_err() {
        let _ = std::fs::remove_file(output);
    }
    result
}

/// Play to EOS, reporting position as progress and stopping on cancel.
fn run(pipeline: &gst::Pipeline, edit: &Edit, control: &Control) -> Result<()> {
    let bus = pipeline.bus().context("no bus")?;
    let span = (edit.end - edit.start).nseconds().max(1) as f64;
    let started = std::time::Instant::now();
    loop {
        if control
            .cancel
            .as_ref()
            .is_some_and(|c| c.load(Ordering::Relaxed))
        {
            bail!("render cancelled");
        }
        match bus.timed_pop_filtered(
            gst::ClockTime::from_mseconds(200),
            &[gst::MessageType::Eos, gst::MessageType::Error],
        ) {
            Some(msg) => match msg.view() {
                gst::MessageView::Error(e) => bail!(
                    "{} from {:?}: {:?}",
                    e.error(),
                    msg.src().map(|s| s.path_string()),
                    e.debug()
                ),
                _ => {
                    if let Some(f) = &control.on_progress {
                        f(1.0);
                    }
                    return Ok(());
                }
            },
            None => {
                if started.elapsed() > Duration::from_secs(600) {
                    bail!("timed out rendering");
                }
                if let Some(f) = &control.on_progress
                    && let Some(pos) = pipeline.query_position::<gst::ClockTime>()
                {
                    let done = pos.saturating_sub(edit.start).nseconds() as f64 / span;
                    f(done.clamp(0.0, 1.0));
                }
            }
        }
    }
}

fn wait(pipeline: &gst::Pipeline, until: gst::MessageType) -> Result<()> {
    let bus = pipeline.bus().context("no bus")?;
    match bus.timed_pop_filtered(
        gst::ClockTime::from_seconds(600),
        &[until, gst::MessageType::Error],
    ) {
        Some(msg) => match msg.view() {
            gst::MessageView::Error(e) => bail!(
                "{} from {:?}: {:?}",
                e.error(),
                msg.src().map(|s| s.path_string()),
                e.debug()
            ),
            _ => Ok(()),
        },
        None => bail!("timed out rendering"),
    }
}

fn aac_encoder(bitrate: i32) -> Result<gst::Element> {
    let name = ["fdkaacenc", "avenc_aac"]
        .into_iter()
        .find(|n| gst::ElementFactory::find(n).is_some())
        .context("no AAC encoder found")?;
    let el = gst::ElementFactory::make(name).build()?;
    el.set_property("bitrate", bitrate);
    Ok(el)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: u64 = 1_000_000;

    #[test]
    fn short_clips_keep_a_high_class() {
        // 10 MB for 10 s: ~7.2 Mb/s, enough for 1080p60.
        let p = plan_size(10 * MB, 10.0, true).unwrap();
        assert_eq!(p.audio_kbps, 128);
        assert_eq!((p.max_height, p.fps), (1080, None));
        assert!(p.video_kbps > 6000, "{p:?}");
    }

    #[test]
    fn longer_clips_step_down_in_size_and_rate() {
        // 10 MB for 30 s: ~2.3 Mb/s of video: 720p at 30 fps.
        let p = plan_size(10 * MB, 30.0, true).unwrap();
        assert_eq!((p.max_height, p.fps), (720, Some(30)));
        // 10 MB for 60 s: ~1.1 Mb/s: 480p, with less audio.
        let p = plan_size(10 * MB, 60.0, true).unwrap();
        assert_eq!((p.max_height, p.fps), (480, Some(30)));
        assert_eq!(p.audio_kbps, 96);
    }

    #[test]
    fn the_plan_fits_the_budget() {
        for secs in [5.0, 30.0, 90.0, 240.0] {
            let p = plan_size(10 * MB, secs, true).unwrap();
            let bytes = (p.video_kbps + p.audio_kbps) as f64 * 1000.0 / 8.0 * secs;
            assert!(
                bytes <= 10.0 * MB as f64 * SIZE_MARGIN + 1000.0,
                "{secs}: {bytes}"
            );
        }
    }

    #[test]
    fn too_long_says_how_long_fits() {
        let e = plan_size(10 * MB, 600.0, true).unwrap_err().to_string();
        assert_eq!(e, "too long to fit in 10 MB: trim it to under 4:38");
        // Without audio, all of it goes to the picture.
        assert_eq!(plan_size(10 * MB, 300.0, false).unwrap().audio_kbps, 0);
    }

    #[test]
    fn scaling_keeps_the_shape_and_pixel_budget() {
        // 16:9 down to 720p.
        assert_eq!(scaled_size(1920, 1080, 720), (1280, 720));
        // Never up.
        assert_eq!(scaled_size(1280, 720, 1080), (1280, 720));
        // Ultrawide: the same pixels as 1280x720, the same shape.
        let (w, h) = scaled_size(3440, 1440, 720);
        assert_eq!((w % 2, h % 2), (0, 0));
        assert!((w as f64 / h as f64 - 3440.0 / 1440.0).abs() < 0.01);
        assert!(w * h <= 1280 * 720 + 4000, "{w}x{h}");
    }
}
