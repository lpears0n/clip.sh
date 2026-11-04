//! Where log lines go. The terminal gets them as before, and so does a file,
//! because the Windows build is a windowed executable with no terminal: a
//! panic there would otherwise close the app without a word.
//!
//! The file is `<local data>/sh.clip.app/logs/clip.sh.log` on Windows
//! (`%LOCALAPPDATA%\sh.clip.app\logs`) and `<local data>/clip.sh/logs` on
//! Linux (`~/.local/share/clip.sh/logs`), rotated once past a few MB.

use std::{
    fs::{File, OpenOptions},
    path::PathBuf,
    sync::Arc,
};

use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

const MAX_BYTES: u64 = 5 * 1024 * 1024;

pub fn log_path() -> PathBuf {
    let app_dir = if cfg!(windows) {
        "sh.clip.app"
    } else {
        "clip.sh"
    };
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(app_dir)
        .join("logs/clip.sh.log")
}

/// Install the subscriber and a panic hook that records the panic before
/// the process dies. Call once, first thing.
pub fn init() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let file = open_log_file();
    let registry = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer());
    match file {
        Some(file) => registry
            .with(fmt::layer().with_writer(file).with_ansi(false))
            .init(),
        None => registry.init(),
    }

    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        tracing::error!(%location, "panic: {info}");
        previous(info);
        #[cfg(windows)]
        show_error(&format!("{info}"));
    }));
}

/// Log `message`, show it to the user where there is no terminal, and exit.
pub fn fatal(message: &str) -> ! {
    tracing::error!("{message}");
    #[cfg(windows)]
    show_error(message);
    std::process::exit(1)
}

/// A native message box, since a windowed executable has nowhere else to
/// put an error.
#[cfg(windows)]
fn show_error(message: &str) {
    use windows::{
        Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW},
        core::HSTRING,
    };
    let text = format!("{message}\n\nThe log is at {}", log_path().display());
    // SAFETY: both strings outlive the call; no window handle is passed.
    unsafe {
        MessageBoxW(
            None,
            &HSTRING::from(text),
            &HSTRING::from("clip.sh"),
            MB_OK | MB_ICONERROR,
        );
    }
}

fn open_log_file() -> Option<Arc<File>> {
    let path = log_path();
    let dir = path.parent()?;
    std::fs::create_dir_all(dir).ok()?;
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_BYTES) {
        let _ = std::fs::rename(&path, path.with_extension("log.1"));
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()
        .map(Arc::new)
}
