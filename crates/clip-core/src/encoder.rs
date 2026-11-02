//! Video codec and hardware/software encoder selection.

use anyhow::{Context, Result, bail};
use gstreamer as gst;
use gstreamer::prelude::*;
use serde::{Deserialize, Serialize};

use crate::capture::VideoConfig;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Codec {
    /// Plays everywhere (browsers, Discord, phones).
    #[default]
    H264,
    /// About the same quality at ~30-40% less size; patchier playback support.
    Hevc,
    /// Best quality per bit; needs a recent GPU and player.
    Av1,
}

impl Codec {
    pub const ALL: [Codec; 3] = [Codec::H264, Codec::Hevc, Codec::Av1];

    /// Parser placed after the encoder (and before muxing).
    pub fn parser(self) -> &'static str {
        match self {
            Codec::H264 => "h264parse",
            Codec::Hevc => "h265parse",
            Codec::Av1 => "av1parse",
        }
    }

    /// Caps forced after the encoder; encoders default to their most basic
    /// profile unless asked for more.
    pub fn caps(self) -> gst::Caps {
        match self {
            Codec::H264 => gst::Caps::builder("video/x-h264")
                .field("profile", "high")
                .build(),
            Codec::Hevc => gst::Caps::builder("video/x-h265")
                .field("profile", "main")
                .build(),
            Codec::Av1 => gst::Caps::new_empty_simple("video/x-av1"),
        }
    }

    /// The codec of an encoded stream's caps.
    pub fn from_caps(caps: &gst::CapsRef) -> Option<Codec> {
        match caps.structure(0)?.name().as_str() {
            "video/x-h264" => Some(Codec::H264),
            "video/x-h265" => Some(Codec::Hevc),
            "video/x-av1" => Some(Codec::Av1),
            _ => None,
        }
    }

    /// Parser configured to repeat stream headers on every keyframe, so any
    /// GOP can start a file.
    pub fn make_parser(self) -> Result<gst::Element> {
        let el = gst::ElementFactory::make(self.parser()).build()?;
        if el.has_property("config-interval") {
            el.set_property("config-interval", -1i32);
        }
        Ok(el)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Encoder {
    /// NVIDIA NVENC (CUDA mode).
    Nvenc,
    /// VA-API (AMD / Intel).
    Vaapi,
    /// Software x264 fallback.
    X264,
}

impl Encoder {
    /// Probe order: hardware first.
    pub const ALL: [Encoder; 3] = [Encoder::Nvenc, Encoder::Vaapi, Encoder::X264];

    /// GStreamer element for `codec`, if this encoder supports it.
    pub fn element(self, codec: Codec) -> Option<&'static str> {
        match (self, codec) {
            (Encoder::Nvenc, Codec::H264) => Some("nvh264enc"),
            (Encoder::Nvenc, Codec::Hevc) => Some("nvh265enc"),
            (Encoder::Nvenc, Codec::Av1) => Some("nvav1enc"),
            (Encoder::Vaapi, Codec::H264) => Some("vah264enc"),
            (Encoder::Vaapi, Codec::Hevc) => Some("vah265enc"),
            (Encoder::Vaapi, Codec::Av1) => Some("vaav1enc"),
            (Encoder::X264, Codec::H264) => Some("x264enc"),
            (Encoder::X264, _) => None,
        }
    }

    /// First encoder that actually encodes `codec` on this machine.
    pub fn probe(codec: Codec) -> Option<Encoder> {
        Self::ALL.into_iter().find(|e| e.works(codec))
    }

    /// Whether the element exists *and* can encode a test frame. Factories can
    /// be registered even when the driver fails at runtime.
    pub fn works(self, codec: Codec) -> bool {
        match self.element(codec) {
            Some(name) if gst::ElementFactory::find(name).is_some() => {
                self.encode_test_frame(codec).is_ok()
            }
            _ => false,
        }
    }

    fn encode_test_frame(self, codec: Codec) -> Result<()> {
        let cfg = VideoConfig {
            codec,
            ..Default::default()
        };
        let pipeline = gst::Pipeline::new();
        let src = gst::ElementFactory::make("videotestsrc")
            .property("num-buffers", 2i32)
            .build()?;
        let caps = gst::ElementFactory::make("capsfilter")
            .property(
                "caps",
                gst::Caps::builder("video/x-raw")
                    .field("format", "BGRx")
                    .field("width", 640i32)
                    .field("height", 360i32)
                    .build(),
            )
            .build()?;
        let sink = gst::ElementFactory::make("fakesink").build()?;

        let mut chain = vec![src, caps];
        chain.extend(self.upload_chain()?);
        chain.push(self.build(&cfg)?);
        chain.push(sink);
        pipeline.add_many(&chain)?;
        gst::Element::link_many(&chain)?;

        pipeline.set_state(gst::State::Playing)?;
        let bus = pipeline.bus().context("pipeline has no bus")?;
        let result = match bus.timed_pop_filtered(
            gst::ClockTime::from_seconds(5),
            &[gst::MessageType::Eos, gst::MessageType::Error],
        ) {
            Some(msg) => match msg.view() {
                gst::MessageView::Eos(_) => Ok(()),
                gst::MessageView::Error(e) => Err(anyhow::anyhow!("{}", e.error())),
                _ => unreachable!(),
            },
            None => Err(anyhow::anyhow!("timed out")),
        };
        pipeline.set_state(gst::State::Null)?;
        result
    }

    /// Elements placed before the encoder to get raw frames into a memory
    /// type/format it accepts. Converters run in passthrough when possible.
    pub fn upload_chain(self) -> Result<Vec<gst::Element>> {
        let name = match self {
            // nvh264enc takes BGRx/RGBx system memory and converts on the GPU.
            Encoder::Nvenc => "videoconvert",
            Encoder::Vaapi => "vapostproc",
            Encoder::X264 => "videoconvert",
        };
        Ok(vec![gst::ElementFactory::make(name).build()?])
    }

    /// GPU memory this encoder reads directly, for the zero-copy path.
    pub fn gpu_memory(self) -> Option<&'static str> {
        match self {
            Encoder::Nvenc => Some("memory:GLMemory"),
            Encoder::Vaapi => Some("memory:VAMemory"),
            Encoder::X264 => None,
        }
    }

    /// Elements that import a DMA-BUF into the encoder's GPU memory and
    /// convert/scale it there. Scaling is driven by a capsfilter placed
    /// after them by the caller.
    pub fn gpu_chain(self) -> Result<Vec<gst::Element>> {
        let names: &[&str] = match self {
            // GL import; nvenc reads GL textures through CUDA interop.
            Encoder::Nvenc => &["glupload", "glcolorscale", "glcolorconvert"],
            // vapostproc imports DMA-BUF, converts and scales in one element.
            Encoder::Vaapi => &["vapostproc"],
            Encoder::X264 => bail!("x264 has no GPU path"),
        };
        names
            .iter()
            .map(|n| {
                gst::ElementFactory::make(n)
                    .build()
                    .with_context(|| format!("{n} missing (gst-plugins-base/bad GL or VA)"))
            })
            .collect()
    }

    /// Configured encoder element. B-frames are disabled so PTS == DTS, which
    /// keeps the replay buffer and remuxing simple; GOP is one second so a clip
    /// can start on a keyframe at any whole second.
    pub fn build(self, cfg: &VideoConfig) -> Result<gst::Element> {
        let gop = cfg.fps;
        let Some(name) = self.element(cfg.codec) else {
            bail!("{self:?} can't encode {:?}", cfg.codec);
        };
        let el = gst::ElementFactory::make(name)
            .build()
            .with_context(|| format!("creating {name}"))?;
        match self {
            Encoder::Nvenc => {
                el.set_property_from_str("rc-mode", "vbr");
                el.set_property_from_str("preset", "p4");
                el.set_property_from_str("tune", "high-quality");
                el.set_property("bitrate", cfg.bitrate_kbps);
                el.set_property("max-bitrate", cfg.bitrate_kbps * 3 / 2);
                el.set_property("gop-size", gop as i32);
                el.set_property("bframes", 0u32);
                el.set_property("zerolatency", true);
            }
            Encoder::Vaapi => {
                el.set_property_from_str("rate-control", "vbr");
                el.set_property("bitrate", cfg.bitrate_kbps);
                el.set_property("key-int-max", gop);
                el.set_property("b-frames", 0u32);
            }
            Encoder::X264 => {
                el.set_property_from_str("tune", "zerolatency");
                el.set_property_from_str("speed-preset", "veryfast");
                el.set_property("bitrate", cfg.bitrate_kbps);
                el.set_property("key-int-max", gop);
            }
        }
        Ok(el)
    }
}
