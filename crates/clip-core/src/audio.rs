//! PipeWire audio capture → AAC pipeline branch.

use anyhow::{Context, Result};
use gstreamer as gst;
use gstreamer::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioSource {
    /// Monitor of the default output device (game/desktop audio).
    Desktop,
    /// An input device by PipeWire node name, or the default input.
    Mic(Option<String>),
}

impl AudioSource {
    pub fn name(&self) -> &'static str {
        match self {
            AudioSource::Desktop => "desktop",
            AudioSource::Mic(_) => "mic",
        }
    }
}

/// AAC so clips play everywhere (browsers, Discord, the webview).
const AAC_ENCODERS: [&str; 2] = ["fdkaacenc", "avenc_aac"];

/// A capture device the user can pick for the mic track.
#[derive(Debug, Clone, Serialize)]
pub struct InputDevice {
    /// PipeWire node name (stable across reboots); pass to `AudioSource::Mic`.
    pub id: String,
    pub name: String,
}

/// Audio inputs (microphones, line-in), excluding output monitors.
pub fn input_devices() -> Vec<InputDevice> {
    let monitor = gst::DeviceMonitor::new();
    monitor.add_filter(Some("Audio/Source"), None);
    if monitor.start().is_err() {
        return vec![];
    }
    let devices = monitor
        .devices()
        .into_iter()
        .filter_map(|d| {
            let props = d.properties()?;
            let id = props.get::<String>("node.name").ok()?;
            let class = props.get::<String>("media.class").unwrap_or_default();
            // Sink monitors also show up as sources.
            if class != "Audio/Source" || id.ends_with(".monitor") {
                return None;
            }
            Some(InputDevice {
                id,
                name: d.display_name().to_string(),
            })
        })
        .collect();
    monitor.stop();
    devices
}

/// Add `pipewiresrc → 48 kHz stereo → AAC → aacparse` to `pipeline` and return
/// the `aacparse` element for the caller to link onwards.
pub fn add_audio_branch(
    pipeline: &gst::Pipeline,
    source: &AudioSource,
    bitrate: i32,
) -> Result<gst::Element> {
    let mut src = gst::ElementFactory::make("pipewiresrc").name(source.name());
    if let AudioSource::Mic(Some(device)) = source {
        src = src.property("target-object", device);
    }
    if *source == AudioSource::Desktop {
        // Without a target, this captures the default sink's monitor and
        // follows it when the default changes.
        src = src.property(
            "stream-properties",
            gst::Structure::builder("props")
                .field("stream.capture.sink", "true")
                .build(),
        );
    }
    let src = src.build()?;
    let convert = gst::ElementFactory::make("audioconvert").build()?;
    let resample = gst::ElementFactory::make("audioresample").build()?;
    let caps = gst::ElementFactory::make("capsfilter")
        .property(
            "caps",
            gst::Caps::builder("audio/x-raw")
                .field("rate", 48_000i32)
                .field("channels", 2i32)
                .build(),
        )
        .build()?;
    let queue = gst::ElementFactory::make("queue")
        .property(
            "max-size-time",
            gst::ClockTime::from_mseconds(500).nseconds(),
        )
        .property("max-size-buffers", 0u32)
        .property("max-size-bytes", 0u32)
        .property_from_str("leaky", "downstream")
        .build()?;
    let enc = aac_encoder(bitrate)?;
    let parse = gst::ElementFactory::make("aacparse").build()?;

    let chain = [src, convert, resample, caps, queue, enc, parse.clone()];
    pipeline.add_many(&chain)?;
    gst::Element::link_many(&chain)?;
    Ok(parse)
}

fn aac_encoder(bitrate: i32) -> Result<gst::Element> {
    let name = AAC_ENCODERS
        .into_iter()
        .find(|n| gst::ElementFactory::find(n).is_some())
        .context("no AAC encoder found (install gst-plugins-bad or gst-libav)")?;
    let el = gst::ElementFactory::make(name).build()?;
    el.set_property("bitrate", bitrate);
    Ok(el)
}

#[cfg(test)]
mod tests {
    #[test]
    fn lists_inputs_without_monitors() {
        crate::init().unwrap();
        let devices = super::input_devices();
        for d in &devices {
            println!("{} — {}", d.id, d.name);
        }
        assert!(devices.iter().all(|d| !d.id.ends_with(".monitor")));
    }
}
