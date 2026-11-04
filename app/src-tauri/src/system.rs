//! Desktop integration: notifications, the save sound, and autostart.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tauri::AppHandle;

use crate::settings::Sound;

/// Argument the autostart entry passes, so login launches can stay in the tray.
pub const AUTOSTART_ARG: &str = "--autostart";

/// Show a desktop notification.
pub async fn notify(app: &AppHandle, summary: &str, body: &str) -> Result<()> {
    imp::notify(app, summary, body).await
}

/// The built-in "clip saved" sound: 0.72 s, mono 16-bit PCM WAV, embedded
/// so both platforms have it with nothing to install. Made from the
/// owner's beep with the trailing silence trimmed, a 80 ms fade-out, and
/// 14.3 dB of gain taken off (about -21 LUFS, peaks at -13.7 dBFS), so it is
/// heard over a game without being loud.
const SAVE_SOUND: &[u8] = include_bytes!("../sounds/clip-saved.wav");

/// Play `sound` at `volume_percent` without blocking. A custom file that
/// has gone missing falls back to the built-in sound; if GStreamer can't
/// play at all, Windows still plays the built-in one through the system.
pub fn play_sound(sound: &Sound, volume_percent: u32) {
    let custom = match sound {
        Sound::Custom { path, .. } if path.is_file() => Some(path.clone()),
        Sound::Custom { path, .. } => {
            tracing::warn!(path = %path.display(), "custom sound is gone; using the built-in one");
            None
        }
        Sound::Builtin => None,
    };
    std::thread::spawn(move || {
        let path = custom.or_else(|| builtin_sound_file().map(Path::to_owned));
        let result = match &path {
            Some(p) => clip_core::sound::play_blocking(p, volume_percent, None),
            None => Err(anyhow::anyhow!("can't write the built-in sound out")),
        };
        if let Err(e) = result {
            tracing::warn!("sound: {e:#}");
            imp::play_builtin_fallback();
        }
    });
}

/// The embedded sound written out once per run, for the player to read.
fn builtin_sound_file() -> Option<&'static Path> {
    static PATH: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    PATH.get_or_init(|| {
        let path = dirs::cache_dir()?.join("clip.sh/clip-saved.wav");
        let current = std::fs::read(&path).is_ok_and(|b| b == SAVE_SOUND);
        if !current {
            std::fs::create_dir_all(path.parent()?).ok()?;
            std::fs::write(&path, SAVE_SOUND).ok()?;
        }
        Some(path)
    })
    .as_deref()
}

/// Where imported sounds are kept: the config folder, so they move with
/// the settings and survive the original file being moved or deleted.
pub fn sounds_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("clip.sh/sounds")
}

/// Check `source` is a short sound and copy it in as `<slot>.<ext>`,
/// replacing an earlier import for the same slot. Returns the copy.
pub fn import_sound(source: &Path, slot: &str) -> Result<PathBuf> {
    import_sound_into(&sounds_dir(), source, slot)
}

fn import_sound_into(dir: &Path, source: &Path, slot: &str) -> Result<PathBuf> {
    clip_core::sound::validate(source)?;
    let ext = source
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .filter(|e| !e.is_empty() && e.chars().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or_else(|| "audio".into());
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    // A new file name per import, so a sound playing from the old copy is
    // never overwritten underneath it; older imports for the slot go.
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let dest = dir.join(format!("{slot}-{stamp}.{ext}"));
    std::fs::copy(source, &dest).with_context(|| format!("copying {}", source.display()))?;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            let ours = p
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(&format!("{slot}-")));
            if ours && p != dest {
                let _ = std::fs::remove_file(p);
            }
        }
    }
    Ok(dest)
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
    use tauri::AppHandle;
    use zbus::zvariant::Value;

    use super::{AUTOSTART_ARG, autostart_file};

    /// GStreamer is the only player on Linux; a failure was logged.
    pub fn play_builtin_fallback() {}

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

    /// The embedded WAV through the system's own player, from memory, for
    /// when GStreamer playback fails.
    pub fn play_builtin_fallback() {
        use windows::{
            Win32::Media::Audio::{PlaySoundW, SND_MEMORY, SND_NODEFAULT, SND_SYNC},
            core::PCWSTR,
        };
        // SAFETY: with SND_MEMORY the "name" is a pointer to a whole RIFF
        // WAVE image; it is static, and SND_SYNC returns once it has played.
        unsafe {
            let _ = PlaySoundW(
                PCWSTR(SAVE_SOUND.as_ptr().cast()),
                None,
                SND_MEMORY | SND_SYNC | SND_NODEFAULT,
            );
        }
    }

    pub async fn notify(app: &AppHandle, summary: &str, body: &str) -> Result<()> {
        app.notification()
            .builder()
            .title(summary)
            .body(body)
            .show()
            .context("showing notification")
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

    /// A plain PCM RIFF WAVE plays with the fewest plugins on any platform.
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

    #[test]
    fn imports_replace_the_previous_one_for_the_slot() {
        clip_core::init().unwrap();
        let dir = std::env::temp_dir().join(format!("clipsh-import-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("beep.WAV");
        std::fs::write(&src, SAVE_SOUND).unwrap();
        let sounds = dir.join("sounds");
        let import = |p: &std::path::Path, slot| super::import_sound_into(&sounds, p, slot);

        let first = import(&src, "clip").unwrap();
        // Imports in the same millisecond would share a name.
        std::thread::sleep(std::time::Duration::from_millis(5));
        let second = import(&src, "clip").unwrap();
        let other = import(&src, "screenshot").unwrap();
        assert!(!first.exists(), "the earlier clip import is removed");
        assert!(second.exists() && other.exists());
        assert_eq!(second.extension().unwrap(), "wav");
        assert_eq!(std::fs::read(&second).unwrap(), SAVE_SOUND);

        let text = dir.join("notes.txt");
        std::fs::write(&text, "no").unwrap();
        assert!(import(&text, "clip").is_err());
        assert!(second.exists(), "a failed import keeps the current sound");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
