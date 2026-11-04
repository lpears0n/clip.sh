//! Screenshots from the replay buffer: decode the newest GOP and keep its
//! last frame. Nothing is tapped while recording, so the capture path pays
//! nothing for this; the decode runs only when a screenshot is asked for.

use std::path::Path;

use anyhow::{Context, Result, bail};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;

use crate::{
    encoder::Codec,
    media::{self, Frame, ImageFormat},
    ring::Clip,
};

/// Decode `clip` (normally [`ReplayBuffer::latest_gop`]) and write its last
/// frame to `path` as PNG. Blocks until the file is written.
///
/// [`ReplayBuffer::latest_gop`]: crate::ring::ReplayBuffer::latest_gop
pub fn write_png(clip: &Clip, path: &Path) -> Result<()> {
    let frame = last_frame(clip)?;
    let png = media::encode_image(&frame, ImageFormat::Png, 0)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    std::fs::write(path, png).with_context(|| format!("writing {}", path.display()))
}

/// The last decodable frame of `clip`'s video track, as packed RGB.
pub(crate) fn last_frame(clip: &Clip) -> Result<Frame> {
    if clip.video.buffers.is_empty() {
        bail!("no video buffered yet");
    }
    let codec = Codec::from_caps(&clip.video.caps).context("unknown video codec")?;
    // Only the newest frame is wanted: the sink keeps one and drops the rest
    // as they arrive, and the decoder is asked not to pace itself.
    let pipeline = gst::parse::launch(&format!(
        "appsrc name=src format=time ! {parser} ! decodebin ! videoconvert \
         ! video/x-raw,format=RGB ! appsink name=sink sync=false max-buffers=1 drop=true",
        parser = codec.parser()
    ))?
    .downcast::<gst::Pipeline>()
    .unwrap();
    let src = pipeline
        .by_name("src")
        .unwrap()
        .downcast::<gst_app::AppSrc>()
        .unwrap();
    src.set_caps(Some(&clip.video.caps));
    src.set_max_bytes(0);
    let sink = pipeline
        .by_name("sink")
        .unwrap()
        .downcast::<gst_app::AppSink>()
        .unwrap();
    let bus = pipeline.bus().context("pipeline has no bus")?;

    let result = (|| {
        // The appsrc is unbounded; every packet is queued up front (shared
        // memory, not copied) so the decode runs flat out.
        for buf in &clip.video.buffers {
            src.push_buffer(buf.clone()).context("queueing packet")?;
        }
        src.end_of_stream().context("ending stream")?;
        pipeline.set_state(gst::State::Playing)?;

        let mut last = None;
        loop {
            match sink.try_pull_sample(gst::ClockTime::from_seconds(10)) {
                Some(s) => last = Some(s),
                None => {
                    if let Some(msg) = bus.pop_filtered(&[gst::MessageType::Error])
                        && let gst::MessageView::Error(e) = msg.view()
                    {
                        bail!(
                            "{} from {:?}: {:?}",
                            e.error(),
                            msg.src().map(|s| s.path_string()),
                            e.debug()
                        );
                    }
                    if sink.is_eos() {
                        break;
                    }
                    bail!("timed out decoding the newest frame");
                }
            }
        }
        let sample = last.context("the newest GOP decoded to no frames")?;
        media::frame_of(&sample)
    })();
    let _ = pipeline.set_state(gst::State::Null);
    result
}
