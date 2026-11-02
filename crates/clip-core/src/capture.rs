//! Screen capture → encoder pipeline branch.
//!
//! Two ways to get frames from PipeWire to the encoder:
//!
//! * **Zero-copy** (`CapturePath::ZeroCopy`): the compositor's DMA-BUF is
//!   imported straight into GPU memory (`glupload` for NVENC, `vapostproc`
//!   for VA-API) and encoded there. Nothing crosses to system memory, so a
//!   game saturating the GPU no longer starves the capture of bandwidth.
//! * **Copy** (`CapturePath::Copy`): frames arrive in system memory and the
//!   encoder's upload chain moves them. Works everywhere; costs a copy per
//!   frame and roughly a third of the frame rate under load.
//!
//! The recorder tries zero-copy first and falls back if negotiation fails.

use std::os::fd::RawFd;

use anyhow::{Context, Result, bail};
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

/// How frames travel from the compositor to the encoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CapturePath {
    /// DMA-BUF import into GPU memory; encoder reads it in place.
    ZeroCopy,
    /// Through system memory.
    Copy,
}

/// Add `pipewiresrc → constant fps → (even size) → encoder → parser` to
/// `pipeline` and return the parser for the caller to link onwards. The
/// parser repeats stream headers on every keyframe so any GOP can start a
/// file.
pub fn add_video_branch(
    pipeline: &gst::Pipeline,
    target: PipewireTarget,
    encoder: Encoder,
    cfg: &VideoConfig,
    path: CapturePath,
) -> Result<gst::Element> {
    if path == CapturePath::ZeroCopy && encoder.gpu_memory().is_none() {
        bail!("{encoder:?} has no zero-copy path");
    }
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
    if path == CapturePath::ZeroCopy && src.has_property("always-copy") {
        // Copying can't map a DMA-BUF; it would hand out empty buffers.
        src.set_property("always-copy", false);
    }

    let rate = gst::ElementFactory::make("videorate")
        .name("rate")
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

    let mut chain = vec![src.clone()];
    match path {
        CapturePath::Copy => {
            // Window streams can have odd sizes, which 4:2:0 encoders reject;
            // trim the odd row/column whenever the size changes.
            let crop = gst::ElementFactory::make("videocrop").build()?;
            {
                let crop = crop.clone();
                on_caps(&src, move |w, h| {
                    crop.set_property("right", w & 1);
                    crop.set_property("bottom", h & 1);
                })?;
            }
            let rate_caps = capsfilter(raw_caps(None, |s| {
                s.set("framerate", gst::Fraction::new(cfg.fps as i32, 1));
            }))?;
            chain.extend([crop, rate, rate_caps, queue]);
            chain.extend(encoder.upload_chain()?);
        }
        CapturePath::ZeroCopy => {
            let memory = encoder.gpu_memory().unwrap();
            // Only DMA-BUF leaves the source; a system-memory offer must fail
            // here so the recorder can fall back instead of silently copying.
            let dmabuf = capsfilter(raw_caps(Some("memory:DMABuf"), |_| {}))?;
            let rate_caps = capsfilter(raw_caps(Some("memory:DMABuf"), |s| {
                s.set("framerate", gst::Fraction::new(cfg.fps as i32, 1));
            }))?;
            chain.extend([dmabuf, rate, rate_caps, queue]);
            chain.extend(encoder.gpu_chain()?);
            // The GPU converter also scales: ask for an even size so 4:2:0
            // encoders accept odd window streams.
            let even = capsfilter(raw_caps(Some(memory), |_| {}))?;
            {
                let even = even.clone();
                on_caps(&src, move |w, h| {
                    even.set_property(
                        "caps",
                        raw_caps(Some(memory), |s| {
                            s.set("width", w & !1);
                            s.set("height", h & !1);
                        }),
                    );
                })?;
            }
            chain.push(even);
        }
    }
    chain.push(enc);
    chain.push(enc_caps);
    chain.push(parse.clone());
    pipeline.add_many(&chain)?;
    gst::Element::link_many(&chain)?;
    Ok(parse)
}

/// `video/x-raw` with an optional memory feature (`memory:DMABuf`,
/// `memory:GLMemory`, …); `None` means system memory. `set` adds fields.
fn raw_caps(feature: Option<&str>, set: impl FnOnce(&mut gst::StructureRef)) -> gst::Caps {
    let mut structure = gst::Structure::new_empty("video/x-raw");
    set(structure.as_mut());
    let mut caps = gst::Caps::new_empty();
    caps.get_mut()
        .unwrap()
        .append_structure_full(structure, feature.map(|f| gst::CapsFeatures::new([f])));
    caps
}

fn capsfilter(caps: gst::Caps) -> Result<gst::Element> {
    Ok(gst::ElementFactory::make("capsfilter")
        .property("caps", caps)
        .build()?)
}

/// Call `f(width, height)` from the source pad whenever the stream's caps
/// change (window resizes).
fn on_caps(src: &gst::Element, f: impl Fn(i32, i32) + Send + Sync + 'static) -> Result<()> {
    src.static_pad("src")
        .context("pipewiresrc has no src pad")?
        .add_probe(gst::PadProbeType::EVENT_DOWNSTREAM, move |_, info| {
            if let Some(gst::PadProbeData::Event(ev)) = &info.data
                && let gst::EventView::Caps(c) = ev.view()
                && let Some(s) = c.caps().structure(0)
            {
                let w = s.get::<i32>("width").unwrap_or(0);
                let h = s.get::<i32>("height").unwrap_or(0);
                if w > 0 && h > 0 {
                    f(w, h);
                }
            }
            gst::PadProbeReturn::Ok
        });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_carry_memory_features() {
        crate::init().unwrap();
        let c = raw_caps(Some("memory:DMABuf"), |s| {
            s.set("framerate", gst::Fraction::new(60, 1))
        });
        assert_eq!(
            c.to_string(),
            "video/x-raw(memory:DMABuf), framerate=(fraction)60/1"
        );
        let c = raw_caps(None, |_| {});
        assert_eq!(c.to_string(), "video/x-raw");
    }
}
