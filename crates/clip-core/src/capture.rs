//! Screen capture → H.264 pipeline branch.

use std::os::fd::RawFd;

use anyhow::{Context, Result};
use gstreamer as gst;
use gstreamer::prelude::*;
use serde::{Deserialize, Serialize};

use crate::encoder::{Codec, Encoder};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoConfig {
    pub fps: u32,
    pub bitrate_kbps: u32,
    #[serde(default)]
    pub codec: Codec,
}

impl Default for VideoConfig {
    fn default() -> Self {
        Self {
            fps: 60,
            bitrate_kbps: 20_000,
            codec: Codec::H264,
        }
    }
}

/// Where `pipewiresrc` reads frames from.
#[derive(Debug, Clone, Copy)]
pub enum PipewireTarget {
    /// A portal-provided PipeWire remote and node id. The fd must stay open
    /// for as long as the pipeline runs.
    Remote { fd: RawFd, node_id: u32 },
    /// An object serial on the session's own PipeWire daemon (KWin streams).
    Serial(u64),
}

/// Add `pipewiresrc → even crop → constant fps → encoder → parser` to
/// `pipeline` and return the parser for the caller to link onwards. The
/// parser repeats stream headers on every keyframe so any GOP can start a
/// file.
pub fn add_video_branch(
    pipeline: &gst::Pipeline,
    target: PipewireTarget,
    encoder: Encoder,
    cfg: &VideoConfig,
) -> Result<gst::Element> {
    let src = gst::ElementFactory::make("pipewiresrc")
        .name("screen")
        .property("do-timestamp", true)
        // Compositors only send frames on damage; resend so static scenes
        // still produce video.
        .property("keepalive-time", 1000 / cfg.fps as i32);
    let src = match target {
        PipewireTarget::Remote { fd, node_id } => {
            src.property("fd", fd).property("path", node_id.to_string())
        }
        PipewireTarget::Serial(serial) => src.property("target-object", serial.to_string()),
    }
    .build()?;
    // Window streams can have odd sizes, which 4:2:0 encoders reject; trim
    // the odd row/column whenever the size changes.
    let crop = gst::ElementFactory::make("videocrop").build()?;
    {
        let crop = crop.clone();
        src.static_pad("src")
            .context("pipewiresrc has no src pad")?
            .add_probe(gst::PadProbeType::EVENT_DOWNSTREAM, move |_, info| {
                if let Some(gst::PadProbeData::Event(ev)) = &info.data
                    && let gst::EventView::Caps(c) = ev.view()
                    && let Some(s) = c.caps().structure(0)
                {
                    let w = s.get::<i32>("width").unwrap_or(0);
                    let h = s.get::<i32>("height").unwrap_or(0);
                    crop.set_property("right", w & 1);
                    crop.set_property("bottom", h & 1);
                }
                gst::PadProbeReturn::Ok
            });
    }
    let rate = gst::ElementFactory::make("videorate")
        .name("rate")
        .build()?;
    let rate_caps = gst::ElementFactory::make("capsfilter")
        .property(
            "caps",
            gst::Caps::builder("video/x-raw")
                .field("framerate", gst::Fraction::new(cfg.fps as i32, 1))
                .build(),
        )
        .build()?;
    let queue = gst::ElementFactory::make("queue")
        .property("max-size-buffers", 4u32)
        .property("max-size-time", 0u64)
        .property("max-size-bytes", 0u32)
        .property_from_str("leaky", "downstream")
        .build()?;
    let enc = encoder.build(cfg)?;
    let enc_caps = gst::ElementFactory::make("capsfilter")
        .property("caps", cfg.codec.caps())
        .build()?;
    let parse = cfg.codec.make_parser()?;

    let mut chain = vec![src, crop, rate, rate_caps, queue];
    chain.extend(encoder.upload_chain()?);
    chain.push(enc);
    chain.push(enc_caps);
    chain.push(parse.clone());
    pipeline.add_many(&chain)?;
    gst::Element::link_many(&chain)?;
    Ok(parse)
}
