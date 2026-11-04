//! Desktop integration: notifications, the save sound, and autostart.

use std::path::PathBuf;

use anyhow::Result;
use tauri::AppHandle;

/// Argument the autostart entry passes, so login launches can stay in the tray.
pub const AUTOSTART_ARG: &str = "--autostart";

/// Show a desktop notification.
pub async fn notify(app: &AppHandle, summary: &str, body: &str) -> Result<()> {
    imp::notify(app, summary, body).await
}

/// The "clip saved" sound: 0.72 s, mono 16-bit PCM WAV, embedded so both
/// platforms play the same file with nothing to install. Made from the
/// owner's beep with the trailing silence trimmed, a 80 ms fade-out, and
/// 14.3 dB of gain taken off (about -21 LUFS, peaks at -13.7 dBFS), so it is
/// heard over a game without being loud.
const SAVE_SOUND: &[u8] = include_bytes!("../sounds/clip-saved.wav");

/// Play the "clip saved" sound without blocking.
pub fn play_save_sound() {
    imp::play_save_sound()
}

/// Add or remove the login entry for this executable.
pub fn set_autostart(enabled: bool) -> Result<()> {
    imp::set_autostart(enabled)
}

#[allow(dead_code)]
fn autostart_file() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("autostart/clip-sh.desktop")
}

#[cfg(target_os = "linux")]
mod imp {
    use std::collections::HashMap;

    use anyhow::{Context, Result};
    use clip_core::gst::{self, prelude::*};
    use tauri::AppHandle;
    use zbus::zvariant::Value;

    use super::{AUTOSTART_ARG, SAVE_SOUND, autostart_file};

    /// Show a desktop notification (org.freedesktop.Notifications).
    pub async fn notify(_app: &AppHandle, summary: &str, body: &str) -> Result<()> {
        let conn = zbus::Connection::session().await?;
        let mut hints: HashMap<&str, Value> = HashMap::new();
        // We play our own sound; don't double up with the server's.
        hints.insert("suppress-sound", Value::Bool(true));
        hints.insert("desktop-entry", Value::Str("clip-sh".into()));
        conn.call_method(
            Some("org.freedesktop.Notifications"),
            "/org/freedesktop/Notifications",
            Some("org.freedesktop.Notifications"),
            "Notify",
            &(
                "clip.sh",
                0u32,
                "media-record",
                summary,
                body,
                Vec::<&str>::new(),
                hints,
                4000i32,
            ),
        )
        .await
        .context("notification service unavailable")?;
        Ok(())
    }

    /// The embedded sound written out once per run, for playbin to read.
    fn sound_file() -> Option<&'static std::path::Path> {
        static PATH: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();
        PATH.get_or_init(|| {
            let path = dirs::cache_dir()?.join("clip.sh/clip-saved.wav");
            let current =
                std::fs::metadata(&path).is_ok_and(|m| m.len() == SAVE_SOUND.len() as u64);
            if !current {
                std::fs::create_dir_all(path.parent()?).ok()?;
                std::fs::write(&path, SAVE_SOUND).ok()?;
            }
            Some(path)
        })
        .as_deref()
    }

    /// Play the "clip saved" sound without blocking.
    pub fn play_save_sound() {
        let Some(path) = sound_file() else {
            return;
        };
        let Ok(uri) = gst::glib::filename_to_uri(path, None) else {
            return;
        };
        std::thread::spawn(move || {
            let Ok(play) = gst::ElementFactory::make("playbin")
                .property("uri", uri.as_str())
                .build()
            else {
                return;
            };
            if play.set_state(gst::State::Playing).is_err() {
                return;
            }
            if let Some(bus) = play.bus() {
                bus.timed_pop_filtered(
                    gst::ClockTime::from_seconds(5),
                    &[gst::MessageType::Eos, gst::MessageType::Error],
                );
            }
            let _ = play.set_state(gst::State::Null);
        });
    }

    /// Add or remove the XDG autostart entry for this executable.
    pub fn set_autostart(enabled: bool) -> Result<()> {
        let file = autostart_file();
        if !enabled {
            if file.exists() {
                std::fs::remove_file(&file)?;
            }
            return Ok(());
        }
        let exe = std::env::current_exe()?;
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(
            &file,
            format!(
                "[Desktop Entry]\n\
                 Type=Application\n\
                 Name=clip.sh\n\
                 Comment=Instant replay game clipping\n\
                 Exec=\"{}\" {AUTOSTART_ARG}\n\
                 Icon=media-record\n\
                 X-GNOME-Autostart-enabled=true\n",
                exe.display()
            ),
        )
        .with_context(|| format!("writing {}", file.display()))
    }
}

#[cfg(windows)]
mod imp {
    use anyhow::{Context, Result};
    use tauri::AppHandle;
    use tauri_plugin_notification::NotificationExt;

    use super::{AUTOSTART_ARG, SAVE_SOUND};

    pub async fn notify(app: &AppHandle, summary: &str, body: &str) -> Result<()> {
        app.notification()
            .builder()
            .title(summary)
            .body(body)
            .show()
            .context("showing notification")
    }

    /// The embedded WAV, played from memory, asynchronously.
    pub fn play_save_sound() {
        use windows::{
            Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT},
            core::PCWSTR,
        };
        // SAFETY: with SND_MEMORY the "name" is a pointer to a whole RIFF
        // WAVE image. It is static, so it outlives the asynchronous playback.
        unsafe {
            let _ = PlaySoundW(
                PCWSTR(SAVE_SOUND.as_ptr().cast()),
                None,
                SND_MEMORY | SND_ASYNC | SND_NODEFAULT,
            );
        }
    }

    /// `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, the per-user
    /// login entry that needs no admin rights.
    pub fn set_autostart(enabled: bool) -> Result<()> {
        use winreg::{RegKey, enums::HKEY_CURRENT_USER};
        let (key, _) = RegKey::predef(HKEY_CURRENT_USER)
            .create_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Run")
            .context("opening the Run key")?;
        if enabled {
            let exe = std::env::current_exe()?;
            key.set_value("clip.sh", &format!("\"{}\" {AUTOSTART_ARG}", exe.display()))
                .context("writing the Run entry")?;
        } else if key.get_value::<String, _>("clip.sh").is_ok() {
            key.delete_value("clip.sh")
                .context("removing the Run entry")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::SAVE_SOUND;

    /// PlaySound's SND_MEMORY wants a plain PCM RIFF WAVE image.
    #[test]
    fn save_sound_is_a_pcm_wav() {
        assert_eq!(&SAVE_SOUND[0..4], b"RIFF");
        assert_eq!(&SAVE_SOUND[8..12], b"WAVE");
        assert_eq!(&SAVE_SOUND[12..16], b"fmt ");
        let format = u16::from_le_bytes([SAVE_SOUND[20], SAVE_SOUND[21]]);
        assert_eq!(format, 1, "PCM");
        let riff_len = u32::from_le_bytes(SAVE_SOUND[4..8].try_into().unwrap()) as usize;
        assert_eq!(riff_len + 8, SAVE_SOUND.len());
    }
}
