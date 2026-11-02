//! Capture, encoding and instant-replay buffering for clip.sh.

pub mod audio;
pub mod auto;
pub mod capture;
pub mod display;
pub mod encoder;
pub mod export;
pub mod game;
pub mod hotkey;
pub mod kwin;
pub mod media;
pub mod portal;
pub mod recorder;
pub mod ring;
pub mod save;

pub use audio::{AudioSource, InputDevice, add_audio_branch, input_devices};
pub use capture::{CapturePath, PipewireTarget, VideoConfig, add_video_branch};
pub use encoder::{Codec, Encoder};
pub use gstreamer as gst;
pub use portal::ScreenSource;
pub use recorder::{CaptureSource, Recorder, RecorderConfig, Status};
pub use ring::{Clip, ReplayBuffer};
pub use save::{clip_path, default_clips_dir, write_mp4};

/// Initialise GStreamer. Safe to call more than once.
pub fn init() -> anyhow::Result<()> {
    gstreamer::init()?;
    Ok(())
}
