//! Audio capture → AAC pipeline branch. PipeWire on Linux (an output's
//! monitor for game audio), WASAPI on Windows (loopback of an output).

use anyhow::{Context, Result};
use gstreamer as gst;
use gstreamer::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioSource {
    /// Everything playing through an output device (game/desktop audio), by
    /// id, or through the default output (which it then follows).
    Desktop(Option<String>),
    /// An input device by id, or the default input.
    Mic(Option<String>),
}

impl AudioSource {
    pub fn name(&self) -> &'static str {
        match self {
            AudioSource::Desktop(_) => "desktop",
            AudioSource::Mic(_) => "mic",
        }
    }

    fn device(&self) -> Option<&str> {
        match self {
            AudioSource::Desktop(d) | AudioSource::Mic(d) => d.as_deref(),
        }
    }

    fn kind(&self) -> DeviceKind {
        match self {
            AudioSource::Desktop(_) => DeviceKind::Output,
            AudioSource::Mic(_) => DeviceKind::Input,
        }
    }
}

/// AAC so clips play everywhere (browsers, Discord, the webview).
const AAC_ENCODERS: [&str; 2] = ["fdkaacenc", "avenc_aac"];

/// An audio device the user can pick: an input for the mic track, or an
/// output whose sound is recorded as the game track.
#[derive(Debug, Clone, Serialize)]
pub struct AudioDevice {
    /// Stable device id (PipeWire node name / WASAPI device id); pass to
    /// [`AudioSource`].
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeviceKind {
    Input,
    Output,
}

/// Audio inputs (microphones, line-in). Output monitors and loopbacks,
/// which some systems list as inputs too, are left out.
pub fn input_devices() -> Vec<AudioDevice> {
    devices(DeviceKind::Input)
}

/// Audio outputs (speakers, headsets, virtual sinks) that can be recorded.
pub fn output_devices() -> Vec<AudioDevice> {
    devices(DeviceKind::Output)
}

fn devices(kind: DeviceKind) -> Vec<AudioDevice> {
    let class = match kind {
        DeviceKind::Input => "Audio/Source",
        DeviceKind::Output => "Audio/Sink",
    };
    let monitor = gst::DeviceMonitor::new();
    monitor.add_filter(Some(class), None);
    if monitor.start().is_err() {
        return vec![];
    }
    let mut devices: Vec<AudioDevice> = monitor
        .devices()
        .into_iter()
        .filter_map(|d| {
            let props = d.properties()?;
            #[cfg(target_os = "linux")]
            let id = {
                let id = props.get::<String>("node.name").ok()?;
                let media_class = props.get::<String>("media.class").unwrap_or_default();
                // Sink monitors also show up as sources.
                if media_class != class || id.ends_with(".monitor") {
                    return None;
                }
                id
            };
            #[cfg(not(target_os = "linux"))]
            let id = {
                // WASAPI lists a loopback "source" for every output.
                if props
                    .get::<bool>("wasapi2.device.loopback")
                    .unwrap_or(false)
                {
                    return None;
                }
                props
                    .get::<String>("device.id")
                    .or_else(|_| props.get::<String>("device.strid"))
                    .ok()?
            };
            Some(AudioDevice {
                id,
                name: d.display_name().to_string(),
            })
        })
        .collect();
    monitor.stop();
    devices.dedup_by(|a, b| a.id == b.id);
    devices
}

/// Add `pipewiresrc → 48 kHz stereo → AAC → aacparse` to `pipeline` and return
/// the `aacparse` element for the caller to link onwards.
pub fn add_audio_branch(
    pipeline: &gst::Pipeline,
    source: &AudioSource,
    bitrate: i32,
) -> Result<gst::Element> {
    let src = make_source(&present(source))?;
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

/// `source`, or the default device of its kind if the one it names isn't
/// connected right now (an unplugged headset must not stop the recording).
fn present(source: &AudioSource) -> AudioSource {
    let Some(id) = source.device() else {
        return source.clone();
    };
    if devices(source.kind()).iter().any(|d| d.id == id) {
        return source.clone();
    }
    tracing::warn!(
        track = source.name(),
        device = id,
        "audio device not connected; recording the default instead"
    );
    match source {
        AudioSource::Desktop(_) => AudioSource::Desktop(None),
        AudioSource::Mic(_) => AudioSource::Mic(None),
    }
}

#[cfg(target_os = "linux")]
fn make_source(source: &AudioSource) -> Result<gst::Element> {
    let mut src = gst::ElementFactory::make("pipewiresrc").name(source.name());
    if let Some(device) = source.device() {
        src = src.property("target-object", device);
    }
    if let AudioSource::Desktop(_) = source {
        // Capture an output's monitor: the targeted sink, or without a
        // target the default sink, following it when the default changes.
        src = src.property(
            "stream-properties",
            gst::Structure::builder("props")
                .field("stream.capture.sink", "true")
                .build(),
        );
    }
    Ok(src.build()?)
}

#[cfg(not(target_os = "linux"))]
fn make_source(source: &AudioSource) -> Result<gst::Element> {
    // wasapi2src: shared-mode WASAPI, low latency, no exclusive grabs.
    let mut src = gst::ElementFactory::make("wasapi2src")
        .name(source.name())
        .property("low-latency", true);
    if let AudioSource::Desktop(_) = source {
        // Loopback of a render device: everything playing through it. With
        // no device named, the default one.
        src = src.property("loopback", true);
    }
    if let Some(device) = source.device() {
        src = src.property("device", device);
    }
    src.build()
        .context("wasapi2src missing (GStreamer wasapi2 plugin)")
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
    use super::*;

    #[test]
    fn lists_inputs_without_monitors() {
        crate::init().unwrap();
        let devices = input_devices();
        for d in &devices {
            println!("in: {} — {}", d.id, d.name);
        }
        assert!(devices.iter().all(|d| !d.id.ends_with(".monitor")));
        for d in output_devices() {
            println!("out: {} — {}", d.id, d.name);
        }
    }

    #[test]
    fn missing_devices_fall_back_to_the_default() {
        crate::init().unwrap();
        let gone = "clip.sh-test-device-that-does-not-exist";
        assert_eq!(
            present(&AudioSource::Desktop(Some(gone.into()))),
            AudioSource::Desktop(None)
        );
        assert_eq!(
            present(&AudioSource::Mic(Some(gone.into()))),
            AudioSource::Mic(None)
        );
        // Defaults stay as they are.
        assert_eq!(
            present(&AudioSource::Desktop(None)),
            AudioSource::Desktop(None)
        );
    }
}
