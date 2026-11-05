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
    /// NVIDIA NVENC. CUDA on Linux; D3D11 on Windows so capture frames never
    /// leave the GPU.
    Nvenc,
    /// VA-API (AMD / Intel on Linux).
    Vaapi,
    /// AMD AMF (Windows).
    Amf,
    /// Intel Quick Sync (Windows).
    Qsv,
    /// Media Foundation (Windows; whatever the GPU vendor driver exposes).
    Mf,
    /// Software x264 fallback.
    X264,
}

/// Set a property if the element has one by that name and takes the value.
/// Property names, types, ranges and enum values drift between GStreamer
/// versions and vendors; a miss costs one setting (logged), never the
/// recording, where `set_property` would panic.
pub(crate) fn set<V: Into<gst::glib::Value>>(el: &gst::Element, name: &str, value: V) {
    use gst::glib::{ParamFlags, ParamSpecInt, ParamSpecInt64, ParamSpecUInt, ParamSpecUInt64};

    let Some(pspec) = el.find_property(name) else {
        tracing::debug!(element = %factory_name(el), property = name, "no such property; skipped");
        return;
    };
    if !pspec.flags().contains(ParamFlags::WRITABLE)
        || pspec.flags().contains(ParamFlags::CONSTRUCT_ONLY)
    {
        tracing::debug!(element = %factory_name(el), property = name, "not writable; skipped");
        return;
    }
    let mut v: gst::glib::Value = value.into();
    if !v.type_().is_a(pspec.value_type()) {
        match v.transform_with_type(pspec.value_type()) {
            Ok(t) => v = t,
            Err(e) => {
                tracing::warn!(element = %factory_name(el), property = name, "not set: {e}");
                return;
            }
        }
    }
    // Out-of-range numbers are clamped (GLib would, but the Rust binding
    // treats a changed value as a programming error).
    if let Some(p) = pspec.downcast_ref::<ParamSpecInt>()
        && let Ok(n) = v.get::<i32>()
    {
        v = n.clamp(p.minimum(), p.maximum()).into();
    } else if let Some(p) = pspec.downcast_ref::<ParamSpecUInt>()
        && let Ok(n) = v.get::<u32>()
    {
        v = n.clamp(p.minimum(), p.maximum()).into();
    } else if let Some(p) = pspec.downcast_ref::<ParamSpecInt64>()
        && let Ok(n) = v.get::<i64>()
    {
        v = n.clamp(p.minimum(), p.maximum()).into();
    } else if let Some(p) = pspec.downcast_ref::<ParamSpecUInt64>()
        && let Ok(n) = v.get::<u64>()
    {
        v = n.clamp(p.minimum(), p.maximum()).into();
    }
    el.set_property_from_value(name, &v);
}

/// Like [`set`], for enum (and other string-parsed) values: an unknown value
/// name is skipped, where `set_property_from_str` would panic.
fn set_str(el: &gst::Element, name: &str, value: &str) {
    use gst::prelude::GstValueExt;

    let Some(pspec) = el.find_property(name) else {
        tracing::debug!(element = %factory_name(el), property = name, "no such property; skipped");
        return;
    };
    match gst::glib::Value::deserialize(value, pspec.value_type()) {
        Ok(v) => set(el, name, v),
        Err(e) => tracing::warn!(
            element = %factory_name(el),
            property = name,
            value,
            "not set: {e}"
        ),
    }
}

fn factory_name(el: &gst::Element) -> String {
    el.factory()
        .map(|f| f.name().to_string())
        .unwrap_or_default()
}

impl Encoder {
    /// Probe order: hardware first. Elements that don't exist on a platform
    /// simply fail the probe.
    pub const ALL: [Encoder; 6] = [
        Encoder::Nvenc,
        Encoder::Amf,
        Encoder::Qsv,
        Encoder::Vaapi,
        Encoder::Mf,
        Encoder::X264,
    ];

    /// GStreamer element for `codec`, if this encoder supports it.
    pub fn element(self, codec: Codec) -> Option<&'static str> {
        match (self, codec) {
            #[cfg(windows)]
            (Encoder::Nvenc, Codec::H264) => Some("nvd3d11h264enc"),
            #[cfg(windows)]
            (Encoder::Nvenc, Codec::Hevc) => Some("nvd3d11h265enc"),
            #[cfg(windows)]
            (Encoder::Nvenc, Codec::Av1) => Some("nvd3d11av1enc"),
            #[cfg(not(windows))]
            (Encoder::Nvenc, Codec::H264) => Some("nvh264enc"),
            #[cfg(not(windows))]
            (Encoder::Nvenc, Codec::Hevc) => Some("nvh265enc"),
            #[cfg(not(windows))]
            (Encoder::Nvenc, Codec::Av1) => Some("nvav1enc"),
            (Encoder::Vaapi, Codec::H264) => Some("vah264enc"),
            (Encoder::Vaapi, Codec::Hevc) => Some("vah265enc"),
            (Encoder::Vaapi, Codec::Av1) => Some("vaav1enc"),
            (Encoder::Amf, Codec::H264) => Some("amfh264enc"),
            (Encoder::Amf, Codec::Hevc) => Some("amfh265enc"),
            (Encoder::Amf, Codec::Av1) => Some("amfav1enc"),
            (Encoder::Qsv, Codec::H264) => Some("qsvh264enc"),
            (Encoder::Qsv, Codec::Hevc) => Some("qsvh265enc"),
            (Encoder::Qsv, Codec::Av1) => Some("qsvav1enc"),
            (Encoder::Mf, Codec::H264) => Some("mfh264enc"),
            (Encoder::Mf, Codec::Hevc) => Some("mfh265enc"),
            (Encoder::Mf, Codec::Av1) => None,
            (Encoder::X264, Codec::H264) => Some("x264enc"),
            (Encoder::X264, _) => None,
        }
    }

    /// First encoder that actually encodes `codec` on this machine.
    pub fn probe(codec: Codec) -> Option<Encoder> {
        Self::probe_excluding(codec, &[])
    }

    /// Like [`probe`](Self::probe), skipping encoders that passed the
    /// probe but then failed while recording.
    pub fn probe_excluding(codec: Codec, skip: &[Encoder]) -> Option<Encoder> {
        Self::ALL
            .into_iter()
            .filter(|e| !skip.contains(e))
            .find(|e| e.works(codec))
    }

    /// Whether a pipeline error `message` came from this encoder's element
    /// (GStreamer names the failing element, e.g. `nvd3d11h264enc3`).
    pub fn is_blamed_in(self, codec: Codec, message: &str) -> bool {
        self.element(codec)
            .is_some_and(|name| message.contains(name))
    }

    /// Whether the element exists *and* can encode a test frame. Factories can
    /// be registered even when the driver fails at runtime.
    pub fn works(self, codec: Codec) -> bool {
        let Some(name) = self.element(codec) else {
            return false;
        };
        if gst::ElementFactory::find(name).is_none() {
            tracing::debug!(element = name, "encoder not available here");
            return false;
        }
        match self.encode_test_frame(codec) {
            Ok(()) => true,
            Err(e) => {
                // Worth a line at info: an encoder that exists but can't run
                // is how a broken driver or install shows itself.
                tracing::info!(element = name, "encoder failed the probe: {e:#}");
                false
            }
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
            // Hardware encoders take BGRx/RGBx/NV12 system memory and upload
            // themselves; videoconvert only runs when a format change is needed.
            Encoder::Nvenc | Encoder::Amf | Encoder::Qsv | Encoder::Mf | Encoder::X264 => {
                "videoconvert"
            }
            Encoder::Vaapi => "vapostproc",
        };
        Ok(vec![gst::ElementFactory::make(name).build()?])
    }

    /// GPU memory this encoder reads directly, for the zero-copy path.
    pub fn gpu_memory(self) -> Option<&'static str> {
        #[cfg(windows)]
        {
            // Every Windows hardware encoder takes D3D11 textures, which is
            // what Windows Graphics Capture produces.
            match self {
                Encoder::Nvenc | Encoder::Amf | Encoder::Qsv | Encoder::Mf => {
                    Some("memory:D3D11Memory")
                }
                Encoder::Vaapi | Encoder::X264 => None,
            }
        }
        #[cfg(not(windows))]
        {
            match self {
                Encoder::Nvenc => Some("memory:GLMemory"),
                Encoder::Vaapi => Some("memory:VAMemory"),
                Encoder::Amf | Encoder::Qsv | Encoder::Mf | Encoder::X264 => None,
            }
        }
    }

    /// Elements that import captured frames into the encoder's GPU memory
    /// and convert/scale them there. Scaling is driven by a capsfilter placed
    /// after them by the caller.
    pub fn gpu_chain(self) -> Result<Vec<gst::Element>> {
        let names: &[&str] = match self {
            #[cfg(windows)]
            Encoder::Nvenc | Encoder::Amf | Encoder::Qsv | Encoder::Mf => &["d3d11convert"],
            #[cfg(not(windows))]
            // GL import; nvenc reads GL textures through CUDA interop.
            Encoder::Nvenc => &["glupload", "glcolorscale", "glcolorconvert"],
            // vapostproc imports DMA-BUF, converts and scales in one element.
            Encoder::Vaapi => &["vapostproc"],
            #[cfg(not(windows))]
            Encoder::Amf | Encoder::Qsv | Encoder::Mf => bail!("{self:?} is Windows-only"),
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
        // Names and values follow GStreamer 1.26; anything an element lacks
        // or spells differently is skipped (and logged) rather than guessed.
        match self {
            Encoder::Nvenc => {
                set_str(&el, "rc-mode", "vbr");
                set_str(&el, "preset", "p4");
                set_str(&el, "tune", "high-quality");
                set(&el, "bitrate", cfg.bitrate_kbps);
                set(&el, "max-bitrate", cfg.bitrate_kbps * 3 / 2);
                set(&el, "gop-size", gop as i32);
                set(&el, "bframes", 0u32);
                set(&el, "zerolatency", true);
            }
            Encoder::Vaapi => {
                set_str(&el, "rate-control", "vbr");
                set(&el, "bitrate", cfg.bitrate_kbps);
                set(&el, "key-int-max", gop);
                set(&el, "b-frames", 0u32);
            }
            Encoder::X264 => {
                set_str(&el, "tune", "zerolatency");
                set_str(&el, "speed-preset", "veryfast");
                set(&el, "bitrate", cfg.bitrate_kbps);
                set(&el, "key-int-max", gop);
            }
            Encoder::Amf => {
                // Latency-constrained VBR; "usage" picks the AMF defaults.
                set_str(&el, "rate-control", "lcvbr");
                set_str(&el, "usage", "low-latency");
                set(&el, "bitrate", cfg.bitrate_kbps);
                set(&el, "max-bitrate", cfg.bitrate_kbps * 3 / 2);
                set(&el, "gop-size", gop as i32);
                set(&el, "b-frames", 0u32);
            }
            Encoder::Qsv => {
                set_str(&el, "rate-control", "vbr");
                set(&el, "bitrate", cfg.bitrate_kbps);
                set(&el, "max-bitrate", cfg.bitrate_kbps * 3 / 2);
                set(&el, "gop-size", gop);
                set(&el, "b-frames", 0u32);
                set(&el, "low-latency", true);
            }
            Encoder::Mf => {
                set_str(&el, "rc-mode", "cbr");
                set(&el, "bitrate", cfg.bitrate_kbps);
                set(&el, "gop-size", gop as i32);
                set(&el, "bframes", 0u32);
                set(&el, "low-latency", true);
            }
        }
        Ok(el)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blames_the_encoder_named_in_an_error() {
        let element = Encoder::Nvenc.element(Codec::H264).unwrap();
        let msg = format!(
            "Could not encode stream. from Some(\"/GstPipeline:pipeline37/X:{element}17\"): \
             Failed to open session"
        );
        assert!(Encoder::Nvenc.is_blamed_in(Codec::H264, &msg));
        assert!(!Encoder::Amf.is_blamed_in(Codec::H264, &msg));
        // A capture failure upstream of the encoder isn't the encoder's.
        let capture = "Internal data stream error. from Some(\"/GstPipeline:p/GstQueue:queue12\"): \
                       streaming stopped, reason not-negotiated (-4)";
        assert!(!Encoder::Nvenc.is_blamed_in(Codec::H264, capture));
    }

    #[test]
    fn probing_skips_encoders_set_aside() {
        crate::init().unwrap();
        // x264 is the last resort everywhere CI runs; skipping it leaves
        // whatever hardware the machine has, or nothing.
        let found = Encoder::probe_excluding(Codec::H264, &[Encoder::X264]);
        assert_ne!(found, Some(Encoder::X264));
        let all_but_x264: Vec<_> = Encoder::ALL
            .into_iter()
            .filter(|e| *e != Encoder::X264)
            .collect();
        assert_eq!(
            Encoder::probe_excluding(Codec::H264, &all_but_x264),
            Some(Encoder::X264)
        );
    }

    /// x264enc stands in for every encoder: the setters must survive a
    /// property the element lacks, a value name it doesn't know, a value
    /// of another numeric type and one outside the allowed range.
    #[test]
    fn property_setters_never_panic() {
        gst::init().unwrap();
        let Ok(el) = gst::ElementFactory::make("x264enc").build() else {
            eprintln!("x264enc missing; skipping");
            return;
        };
        set(&el, "no-such-property", 1u32);
        set_str(&el, "no-such-property", "x");
        set_str(&el, "tune", "not-a-tune");
        set_str(&el, "speed-preset", "veryfast");
        set(&el, "key-int-max", 60u32);
        set(&el, "bitrate", 5000i32); // property is uint
        set(&el, "bitrate", u32::MAX); // above the maximum: clamped
        set(&el, "threads", -5i32); // uint property given a negative: clamped

        assert_eq!(el.property::<u32>("key-int-max"), 60);
        let preset: String = el
            .property_value("speed-preset")
            .serialize()
            .unwrap()
            .into();
        assert_eq!(preset, "veryfast");
        assert!(el.property::<u32>("bitrate") >= 5000);
    }
}
