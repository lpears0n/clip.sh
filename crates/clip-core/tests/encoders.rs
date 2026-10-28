//! Requires GStreamer plugins on the host; reports which encoders work.

use clip_core::Encoder;

#[test]
fn software_encoder_works() {
    clip_core::init().unwrap();
    assert!(
        Encoder::X264.works(clip_core::Codec::H264),
        "x264enc should always be usable"
    );
}

#[test]
fn probe_finds_an_encoder() {
    clip_core::init().unwrap();
    for e in Encoder::ALL {
        for c in clip_core::Codec::ALL {
            println!("{e:?} {c:?}: {}", e.works(c));
        }
    }
    assert!(Encoder::probe(clip_core::Codec::H264).is_some());
}
