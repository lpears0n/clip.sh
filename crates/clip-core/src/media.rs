//! Inspect saved clips: duration and a JPEG thumbnail.

use std::path::Path;

use anyhow::{Context, Result, bail};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;

pub const THUMB_WIDTH: i32 = 1280;

pub struct ClipInfo {
    pub duration: gst::ClockTime,
    pub audio_tracks: usize,
    /// JPEG bytes, `THUMB_WIDTH` wide, aspect preserved.
    pub thumbnail: Vec<u8>,
}

/// Decode the frame about one second in (or the first, for shorter clips).
pub fn inspect(path: &Path) -> Result<ClipInfo> {
    let uri = gst::glib::filename_to_uri(path, None)?;
    let pipeline = gst::parse::launch(&format!(
        "uridecodebin uri=\"{uri}\" name=dec ! videoconvert ! videoscale \
         ! video/x-raw,width={THUMB_WIDTH},pixel-aspect-ratio=1/1 \
         ! jpegenc quality=80 ! appsink name=sink max-buffers=1"
    ))?
    .downcast::<gst::Pipeline>()
    .unwrap();
    // Audio pads are left unlinked (uridecodebin only needs video here), but
    // counted.
    let audio = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    {
        let audio = audio.clone();
        pipeline
            .by_name("dec")
            .unwrap()
            .connect_pad_added(move |_, pad| {
                if pad
                    .current_caps()
                    .or_else(|| Some(pad.query_caps(None)))
                    .and_then(|c| c.structure(0).map(|s| s.name().starts_with("audio/")))
                    .unwrap_or(false)
                {
                    audio.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
            });
    }
    let sink = pipeline
        .by_name("sink")
        .unwrap()
        .downcast::<gst_app::AppSink>()
        .unwrap();

    let result = (|| {
        pipeline.set_state(gst::State::Paused)?;
        wait_preroll(&pipeline)?;
        let duration = pipeline
            .query_duration::<gst::ClockTime>()
            .context("clip has no duration")?;
        let at = gst::ClockTime::from_seconds(1).min(duration / 2);
        if at > gst::ClockTime::ZERO {
            pipeline.seek_simple(gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT, at)?;
            wait_preroll(&pipeline)?;
        }
        let sample = sink
            .try_pull_preroll(gst::ClockTime::from_seconds(5))
            .context("no frame decoded")?;
        let buf = sample.buffer().context("empty frame")?;
        let thumbnail = buf.map_readable()?.to_vec();
        Ok(ClipInfo {
            duration,
            audio_tracks: audio.load(std::sync::atomic::Ordering::Relaxed),
            thumbnail,
        })
    })();
    let _ = pipeline.set_state(gst::State::Null);
    result
}

fn wait_preroll(pipeline: &gst::Pipeline) -> Result<()> {
    let bus = pipeline.bus().context("no bus")?;
    match bus.timed_pop_filtered(
        gst::ClockTime::from_seconds(10),
        &[gst::MessageType::AsyncDone, gst::MessageType::Error],
    ) {
        Some(msg) => match msg.view() {
            gst::MessageView::AsyncDone(_) => Ok(()),
            gst::MessageView::Error(e) => bail!("{}", e.error()),
            _ => unreachable!(),
        },
        None => bail!("timed out opening clip"),
    }
}
