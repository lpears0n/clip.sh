//! Desktop integration: notifications, the save sound, and autostart.

use std::{collections::HashMap, path::PathBuf};

use anyhow::{Context, Result};
use clip_core::gst::{self, prelude::*};
use zbus::zvariant::Value;

/// Show a desktop notification (org.freedesktop.Notifications).
pub async fn notify(summary: &str, body: &str) -> Result<()> {
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

const SOUNDS: [&str; 3] = [
    "/usr/share/sounds/freedesktop/stereo/camera-shutter.oga",
    "/usr/share/sounds/freedesktop/stereo/screen-capture.oga",
    "/usr/share/sounds/freedesktop/stereo/complete.oga",
];

/// Play the "clip saved" sound without blocking.
pub fn play_save_sound() {
    let Some(path) = SOUNDS.iter().find(|p| std::path::Path::new(p).exists()) else {
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

/// Argument the autostart entry passes, so login launches can stay in the tray.
pub const AUTOSTART_ARG: &str = "--autostart";

fn autostart_file() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("autostart/clip-sh.desktop")
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
