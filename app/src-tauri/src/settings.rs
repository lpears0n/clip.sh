//! User settings, stored as JSON in `$XDG_CONFIG_HOME/clip.sh/settings.json`.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clip_core::{AudioSource, Codec, Encoder, RecorderConfig, VideoConfig, auto::Backend};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    // General
    pub launch_at_login: bool,
    /// When launched at login, stay in the tray instead of opening the window.
    pub start_in_tray: bool,
    /// Closing the window keeps clip.sh running in the tray (otherwise quits).
    pub close_to_tray: bool,
    pub notify_on_save: bool,
    pub sound_on_save: bool,

    // Recording
    pub replay_seconds: u32,
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub codec: Codec,
    /// `None` picks the best working encoder for the codec.
    pub encoder: Option<Encoder>,
    pub cursor: bool,
    /// Capture straight into GPU memory (DMA-BUF) when the driver allows;
    /// falls back to copying through system memory by itself.
    pub zero_copy: bool,

    // Audio
    pub desktop_audio: bool,
    pub mic: bool,
    /// PipeWire node name; `None` = system default input.
    pub mic_device: Option<String>,
    pub audio_bitrate_kbps: u32,

    /// Capture backend: `None` = KWin window capture when available, else the
    /// screen-cast portal. Applies on the next start of clip.sh.
    pub capture_backend: Option<Backend>,

    /// Global "save clip" hotkey, e.g. `F8` or `Ctrl+Shift+S`.
    pub hotkey: String,
    pub clips_dir: PathBuf,
    /// Soft cap on the clips folder in GB; `None` = no limit.
    pub storage_limit_gb: Option<f64>,
    /// When enforcing the limit, never delete clips made in the trimmer.
    pub protect_edits: bool,
    /// Deleted clips go to the desktop trash instead of being erased.
    pub use_trash: bool,
    /// Show the reserved ad slots. Not exposed in the UI; set it by hand to
    /// preview the layout.
    pub ads: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            launch_at_login: false,
            start_in_tray: true,
            close_to_tray: true,
            notify_on_save: true,
            sound_on_save: true,
            replay_seconds: 30,
            fps: 60,
            bitrate_kbps: 20_000,
            codec: Codec::H264,
            encoder: None,
            cursor: true,
            zero_copy: true,
            desktop_audio: true,
            mic: false,
            mic_device: None,
            audio_bitrate_kbps: 160,
            capture_backend: None,
            hotkey: "F8".into(),
            clips_dir: clip_core::default_clips_dir(),
            storage_limit_gb: None,
            protect_edits: true,
            use_trash: true,
            ads: false,
        }
    }
}

impl Settings {
    fn path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("clip.sh/settings.json")
    }

    /// Load settings, falling back to defaults if the file is missing or bad.
    pub fn load() -> Self {
        let path = Self::path();
        match std::fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
                tracing::warn!("ignoring invalid {}: {e}", path.display());
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, serde_json::to_string_pretty(self)?)
            .with_context(|| format!("writing {}", path.display()))
    }

    pub fn recorder_config(&self) -> RecorderConfig {
        let mut audio = Vec::new();
        if self.desktop_audio {
            audio.push(AudioSource::Desktop);
        }
        if self.mic {
            audio.push(AudioSource::Mic(self.mic_device.clone()));
        }
        RecorderConfig {
            video: self.video_config(),
            encoder: self.encoder,
            replay_seconds: self.replay_seconds.clamp(5, 600),
            audio,
            audio_bitrate: (self.audio_bitrate_kbps.clamp(64, 320) * 1000) as i32,
            cursor: self.cursor,
            zero_copy: self.zero_copy,
        }
    }

    pub fn storage_policy(&self) -> crate::storage::Policy {
        crate::storage::Policy {
            limit_bytes: self.storage_limit_gb.map(crate::storage::gb),
            protect_edits: self.protect_edits,
            use_trash: self.use_trash,
        }
    }

    pub fn video_config(&self) -> VideoConfig {
        VideoConfig {
            fps: self.fps.clamp(24, 480),
            bitrate_kbps: self.bitrate_kbps.clamp(2_000, 100_000),
            codec: self.codec,
        }
    }
}
