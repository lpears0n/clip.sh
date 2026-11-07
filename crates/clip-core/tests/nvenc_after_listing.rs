//! Listing the encoders for Settings must leave NVENC working for the rest
//! of the process. Test-encoding with Media Foundation didn't: every NVENC
//! session after it failed (`NV_ENC_ERR_INVALID_VERSION`), so the next game
//! recorded on another encoder until Reruns restarted. A test binary of its
//! own, since it is about what this process has loaded. Skipped without an
//! NVIDIA GPU.

use clip_core::{Codec, Encoder};

#[test]
fn listing_encoders_leaves_nvenc_working() {
    clip_core::init().unwrap();
    if !Encoder::Nvenc.works(Codec::H264) {
        eprintln!("NVENC doesn't work here; skipped");
        return;
    }
    for e in Encoder::ALL {
        for c in Codec::ALL {
            println!("{e:?} {c:?}: {}", e.offered(c));
        }
    }
    assert!(
        Encoder::Nvenc.works(Codec::H264),
        "NVENC still opens a session after the encoders were listed"
    );
}
