//! Short notification sounds: the built-in beep or a file the user picked.
//! Played through GStreamer on every platform, so any common format works
//! (WAV, MP3, OGG Vorbis, FLAC, Opus) and the volume is ours to set. Nothing
//! here runs while recording: a sound is a few hundred milliseconds of a
//! separate pipeline after a save.

use std::path::Path;

use anyhow::{Context, Result, bail};
use gstreamer as gst;
use gstreamer::prelude::*;

/// Longest sound accepted, and the most of any sound that is played.
pub const MAX_SECONDS: u64 = 10;

/// Highest volume, in percent of the file's own level.
pub const MAX_VOLUME_PERCENT: u32 = 200;

/// Play `path` at `volume_percent` (100 = the file as it is) without
/// blocking. Failures are logged.
pub fn play(path: &Path, volume_percent: u32) {
    let path = path.to_owned();
    std::thread::spawn(move || {
        if let Err(e) = play_blocking(&path, volume_percent, None) {
            tracing::warn!(path = %path.display(), "sound: {e:#}");
        }
    });
}

/// Play `path` and return when it ends (or after [`MAX_SECONDS`]). `sink`
/// replaces the system audio output (tests use a fakesink).
pub fn play_blocking(path: &Path, volume_percent: u32, sink: Option<gst::Element>) -> Result<()> {
    let uri = gst::glib::filename_to_uri(path, None)?;
    let play = gst::ElementFactory::make("playbin")
        .property("uri", uri.as_str())
        .property(
            "volume",
            f64::from(volume_percent.min(MAX_VOLUME_PERCENT)) / 100.0,
        )
        .build()
        .context("playbin missing (GStreamer playback plugin)")?;
    // Audio only, so a cover image in an MP3 never opens a window, and the
    // volume is applied in software rather than to the device.
    play.set_property_from_str("flags", "audio+soft-volume");
    if let Some(sink) = sink {
        play.set_property("audio-sink", sink);
    }
    let bus = play.bus().context("playbin has no bus")?;
    play.set_state(gst::State::Playing)
        .context("can't start playback")?;
    let result = match bus.timed_pop_filtered(
        gst::ClockTime::from_seconds(MAX_SECONDS),
        &[gst::MessageType::Eos, gst::MessageType::Error],
    ) {
        Some(msg) => match msg.view() {
            gst::MessageView::Error(e) => Err(anyhow::anyhow!("{}", e.error())),
            _ => Ok(()),
        },
        // Longer than allowed: cut it off.
        None => Ok(()),
    };
    let _ = play.set_state(gst::State::Null);
    result
}

/// How long the sound in `path` lasts. Fails if it has no audio GStreamer
/// can decode.
pub fn duration(path: &Path) -> Result<gst::ClockTime> {
    let uri = gst::glib::filename_to_uri(path, None)?;
    let pipeline = gst::parse::launch(&format!(
        "uridecodebin uri=\"{uri}\" caps=audio/x-raw ! audioconvert ! fakesink"
    ))?
    .downcast::<gst::Pipeline>()
    .unwrap();
    let bus = pipeline.bus().context("pipeline has no bus")?;
    let result = (|| {
        pipeline.set_state(gst::State::Paused)?;
        match bus.timed_pop_filtered(
            gst::ClockTime::from_seconds(5),
            &[gst::MessageType::AsyncDone, gst::MessageType::Error],
        ) {
            Some(msg) => {
                if let gst::MessageView::Error(e) = msg.view() {
                    bail!("not a sound clip.sh can play ({})", e.error());
                }
            }
            None => bail!("timed out reading the file"),
        }
        pipeline
            .query_duration::<gst::ClockTime>()
            .context("the file doesn't say how long it is")
    })();
    let _ = pipeline.set_state(gst::State::Null);
    result
}

/// Check that `path` is a sound short enough to use for a notification.
pub fn validate(path: &Path) -> Result<gst::ClockTime> {
    let d = duration(path)?;
    if d > gst::ClockTime::from_seconds(MAX_SECONDS) {
        bail!(
            "that sound is {:.0} seconds long; pick one of {MAX_SECONDS} seconds or less",
            d.seconds_f64()
        );
    }
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode `secs` of a test tone with `encoder` (e.g. "wavenc") to a file.
    fn tone(dir: &Path, name: &str, encoder: &str, secs: f64) -> std::path::PathBuf {
        gst::init().unwrap();
        let path = dir.join(name);
        let samples = (48_000.0 * secs) as u64;
        let pipeline = gst::parse::launch(&format!(
            "audiotestsrc num-buffers={} samplesperbuffer=480 ! audio/x-raw,rate=48000,channels=1 \
             ! audioconvert ! {encoder} ! filesink name=out",
            samples / 480,
        ))
        .unwrap()
        .downcast::<gst::Bin>()
        .unwrap();
        // Set as a property, not in the launch string: the launch syntax
        // eats the backslashes of a Windows path.
        pipeline
            .by_name("out")
            .unwrap()
            .set_property("location", path.to_str().unwrap());
        pipeline.set_state(gst::State::Playing).unwrap();
        let bus = pipeline.bus().unwrap();
        let msg = bus
            .timed_pop_filtered(
                gst::ClockTime::from_seconds(20),
                &[gst::MessageType::Eos, gst::MessageType::Error],
            )
            .expect("encoding the test tone timed out");
        assert!(matches!(msg.view(), gst::MessageView::Eos(_)), "{msg:?}");
        pipeline.set_state(gst::State::Null).unwrap();
        path
    }

    fn tempdir(tag: &str) -> std::path::PathBuf {
        gst::init().unwrap();
        let d = std::env::temp_dir().join(format!("clipsh-sound-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn reads_durations_of_common_formats() {
        let dir = tempdir("formats");
        let wav = tone(&dir, "a.wav", "wavenc", 0.5);
        let d = duration(&wav).unwrap();
        assert!((0.45..0.55).contains(&d.seconds_f64()), "{d}");
        if gst::ElementFactory::find("vorbisenc").is_some() {
            let ogg = tone(&dir, "a.ogg", "vorbisenc ! oggmux", 1.0);
            let d = duration(&ogg).unwrap();
            assert!((0.9..1.1).contains(&d.seconds_f64()), "{d}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rejects_non_audio_and_long_files() {
        let dir = tempdir("reject");
        let text = dir.join("notes.txt");
        std::fs::write(&text, "not a sound").unwrap();
        assert!(validate(&text).is_err());
        let long = tone(&dir, "long.wav", "wavenc", MAX_SECONDS as f64 + 1.0);
        let err = validate(&long).unwrap_err().to_string();
        assert!(err.contains("seconds or less"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn plays_to_the_end() {
        let dir = tempdir("play");
        let wav = tone(&dir, "a.wav", "wavenc", 0.3);
        let sink = gst::ElementFactory::make("fakesink")
            .property("sync", true)
            .build()
            .unwrap();
        let started = std::time::Instant::now();
        play_blocking(&wav, 50, Some(sink)).unwrap();
        // Played in real time, not skipped.
        assert!(started.elapsed() >= std::time::Duration::from_millis(250));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
