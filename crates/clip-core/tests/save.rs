//! End-to-end save path without the portal: test sources → encoders → ring
//! buffer → MP4, then verify the file with ffprobe.

use std::{path::Path, process::Command};

use clip_core::{Encoder, ReplayBuffer, VideoConfig, ring::running_time_buffer, write_mp4};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;

/// Encode `seconds` of test video (and audio) into a ring with `window`.
fn fill_ring(seconds: u64, window: u64) -> ReplayBuffer {
    clip_core::init().unwrap();
    let fps = 30;
    let pipeline = gst::parse::launch(&format!(
        "videotestsrc num-buffers={n} is-live=false ! video/x-raw,width=320,height=240,framerate={fps}/1,format=BGRx \
         ! videoconvert ! x264enc name=enc ! video/x-h264,profile=high ! h264parse config-interval=-1 ! appsink name=v sync=false \
         audiotestsrc num-buffers={an} samplesperbuffer=1024 ! audio/x-raw,rate=48000,channels=2 \
         ! audioconvert ! fdkaacenc ! aacparse ! appsink name=a sync=false",
        n = seconds * fps,
        an = seconds * 48_000 / 1024,
    ))
    .unwrap()
    .downcast::<gst::Pipeline>()
    .unwrap();
    // Same encoder settings as the recorder (1s GOP, no B-frames).
    let cfg = VideoConfig {
        fps: fps as u32,
        bitrate_kbps: 2_000,
        ..Default::default()
    };
    let real = Encoder::X264.build(&cfg).unwrap();
    let enc = pipeline.by_name("enc").unwrap();
    for p in ["key-int-max", "bitrate"] {
        enc.set_property_from_value(p, &real.property_value(p));
    }
    enc.set_property_from_str("tune", "zerolatency");

    let mut ring = ReplayBuffer::new(gst::ClockTime::from_seconds(window), 1);
    let v = pipeline
        .by_name("v")
        .unwrap()
        .downcast::<gst_app::AppSink>()
        .unwrap();
    let a = pipeline
        .by_name("a")
        .unwrap()
        .downcast::<gst_app::AppSink>()
        .unwrap();
    pipeline.set_state(gst::State::Playing).unwrap();

    // Pull both sinks in lockstep so neither stalls the other.
    let (mut v_done, mut a_done) = (false, false);
    while !(v_done && a_done) {
        if !v_done {
            match v.try_pull_sample(gst::ClockTime::from_mseconds(10)) {
                Some(s) => {
                    ring.video.set_caps(s.caps_owned().unwrap());
                    ring.push_video(running_time_buffer(&s).unwrap());
                }
                None => v_done = v.is_eos(),
            }
        }
        if !a_done {
            match a.try_pull_sample(gst::ClockTime::from_mseconds(10)) {
                Some(s) => {
                    ring.audio[0].set_caps(s.caps_owned().unwrap());
                    ring.push_audio(0, running_time_buffer(&s).unwrap());
                }
                None => a_done = a.is_eos(),
            }
        }
    }
    pipeline.set_state(gst::State::Null).unwrap();
    ring
}

fn ffprobe(path: &Path, entries: &str) -> String {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", entries, "-of", "csv=p=0"])
        .arg(path)
        .output()
        .expect("ffprobe not installed");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn saves_last_n_seconds_as_mp4() {
    let ring = fill_ring(8, 5);
    assert!(ring.buffered() >= gst::ClockTime::from_seconds(5));

    let clip = ring.snapshot(gst::ClockTime::from_seconds(3)).unwrap();
    let dir = std::env::temp_dir().join(format!("clipsh-test-{}", std::process::id()));
    let path = dir.join("clip.mp4");
    write_mp4(&clip, &path).unwrap();

    let streams = ffprobe(&path, "stream=codec_type,codec_name,profile,start_time");
    let lines: Vec<_> = streams.lines().collect();
    assert_eq!(lines.len(), 2, "{streams}");
    assert!(lines[0].starts_with("h264,High,video"), "{streams}");
    assert!(lines[1].starts_with("aac,LC,audio"), "{streams}");

    let duration: f64 = ffprobe(&path, "format=duration").trim().parse().unwrap();
    assert!((2.9..=3.2).contains(&duration), "duration {duration}");

    let info = clip_core::media::inspect(&path).unwrap();
    assert!((2.9..=3.2).contains(&info.duration.seconds_f64()));
    assert_eq!(&info.thumbnail[..2], &[0xFF, 0xD8], "not a JPEG");

    assert_eq!(info.audio_tracks, 1);

    // Trim 0.5s..2.0s with the audio halved.
    let edited = dir.join("edit.mp4");
    let edit = clip_core::export::Edit {
        start: gst::ClockTime::from_mseconds(500),
        end: gst::ClockTime::from_mseconds(2000),
        volumes: vec![0.5],
    };
    let cfg = VideoConfig {
        fps: 30,
        bitrate_kbps: 2_000,
        ..Default::default()
    };
    clip_core::export::render(&path, &edited, &edit, Encoder::X264, &cfg).unwrap();
    let streams = ffprobe(&edited, "stream=codec_name,codec_type");
    assert_eq!(
        streams.lines().collect::<Vec<_>>(),
        ["h264,video", "aac,audio"],
        "{streams}"
    );
    let duration: f64 = ffprobe(&edited, "format=duration").trim().parse().unwrap();
    assert!(
        (1.4..=1.6).contains(&duration),
        "edited duration {duration}"
    );

    std::fs::remove_dir_all(dir).unwrap();
}

/// Every codec the machine can hardware-encode survives ring → MP4.
#[test]
fn saves_every_supported_codec() {
    use clip_core::Codec;

    clip_core::init().unwrap();
    for codec in Codec::ALL {
        let Some(encoder) = Encoder::probe(codec) else {
            println!("{codec:?}: no encoder, skipped");
            continue;
        };
        let cfg = VideoConfig {
            fps: 60,
            bitrate_kbps: 100_000,
            codec,
        };
        let pipeline = gst::Pipeline::new();
        let src = gst::ElementFactory::make("videotestsrc")
            .property("num-buffers", 180i32)
            .property_from_str("pattern", "ball")
            .build()
            .unwrap();
        let caps = gst::ElementFactory::make("capsfilter")
            .property(
                "caps",
                gst::Caps::builder("video/x-raw")
                    .field("format", "BGRx")
                    .field("width", 1280i32)
                    .field("height", 720i32)
                    .field("framerate", gst::Fraction::new(60, 1))
                    .build(),
            )
            .build()
            .unwrap();
        let sink = gst_app::AppSink::builder().sync(false).build();
        let mut chain = vec![src, caps];
        chain.extend(encoder.upload_chain().unwrap());
        chain.push(encoder.build(&cfg).unwrap());
        chain.push(
            gst::ElementFactory::make("capsfilter")
                .property("caps", codec.caps())
                .build()
                .unwrap(),
        );
        chain.push(codec.make_parser().unwrap());
        chain.push(sink.clone().upcast());
        pipeline.add_many(&chain).unwrap();
        gst::Element::link_many(&chain).unwrap();

        let mut ring = ReplayBuffer::new(gst::ClockTime::from_seconds(30), 0);
        pipeline.set_state(gst::State::Playing).unwrap();
        while let Some(s) = sink.try_pull_sample(gst::ClockTime::from_seconds(5)) {
            ring.video.set_caps(s.caps_owned().unwrap());
            ring.push_video(running_time_buffer(&s).unwrap());
        }
        pipeline.set_state(gst::State::Null).unwrap();

        let clip = ring.snapshot(gst::ClockTime::from_seconds(2)).unwrap();
        let path =
            std::env::temp_dir().join(format!("clipsh-codec-{}-{codec:?}.mp4", std::process::id()));
        write_mp4(&clip, &path).unwrap();
        let probe = ffprobe(&path, "stream=codec_name,width,height");
        let expected = match codec {
            Codec::H264 => "h264",
            Codec::Hevc => "hevc",
            Codec::Av1 => "av1",
        };
        assert_eq!(
            probe.trim(),
            format!("{expected},1280,720"),
            "{codec:?} via {encoder:?}"
        );
        println!("{codec:?} via {encoder:?}: ok");
        std::fs::remove_file(path).unwrap();
    }
}
