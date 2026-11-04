//! The in-game popup: "Ready to record" when a game starts recording,
//! "Clip saved" / "Screenshot saved" on a save, "Moment marked" on a
//! marker. Windows only.
//!
//! It is a small transparent window kept on top of the game, not something
//! drawn into the game: hooking a game's rendering (the way Discord's or
//! Steam's overlays do) is what anti-cheat flags. The window never takes
//! focus (a focus change would knock a fullscreen game out of fullscreen),
//! lets clicks through, and is only shown for a few seconds, since any
//! window over a fullscreen game makes Windows compose the game while it is
//! visible. Recording captures only the game's window, so the popup never
//! appears in clips.
//!
//! Linux has no portable way to keep a window above a game on Wayland; the
//! desktop notification serves there.

use serde::Serialize;
use tauri::AppHandle;

/// Event the overlay page listens for.
#[cfg_attr(not(windows), allow(dead_code))]
pub const SHOW_EVENT: &str = "overlay-show";

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Ready,
    Saved,
    Screenshot,
    Marked,
}

#[derive(Debug, Clone, Serialize)]
pub struct Popup {
    pub kind: Kind,
    pub title: String,
    pub detail: String,
    /// Hotkey shown as a key cap ("Press F8 to clip").
    pub key: Option<String>,
    /// How long the window stays up, in milliseconds.
    pub ms: u64,
}

impl Popup {
    pub fn ready(game: &str, hotkey: &str) -> Self {
        Self {
            kind: Kind::Ready,
            title: "Ready to record".into(),
            detail: game.into(),
            key: (!hotkey.is_empty()).then(|| hotkey.into()),
            ms: 3500,
        }
    }

    pub fn saved(kind: Kind, game: &str) -> Self {
        Self {
            kind,
            title: match kind {
                Kind::Screenshot => "Screenshot saved",
                Kind::Marked => "Moment marked",
                _ => "Clip saved",
            }
            .into(),
            detail: game.into(),
            key: None,
            ms: 2500,
        }
    }

    /// Short: a marker is pressed mid-fight.
    pub fn marked(game: &str) -> Self {
        Self {
            ms: 1500,
            ..Self::saved(Kind::Marked, game)
        }
    }
}

pub use imp::{create, show};

#[cfg(windows)]
mod imp {
    use std::{
        sync::atomic::{AtomicU64, Ordering},
        time::Duration,
    };

    use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, WebviewUrl, WebviewWindowBuilder};
    use windows::Win32::{
        Foundation::{HWND, POINT, RECT},
        Graphics::Gdi::ClientToScreen,
        UI::WindowsAndMessaging::{
            GetClientRect, GetForegroundWindow, HWND_TOPMOST, SW_HIDE, SW_SHOWNOACTIVATE,
            SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SetWindowPos, ShowWindow,
        },
    };

    use super::{Popup, SHOW_EVENT};

    const LABEL: &str = "overlay";
    /// Logical size of the window: room for the card and its fade.
    const WIDTH: f64 = 380.0;
    const HEIGHT: f64 = 72.0;
    /// Gap from the game's top-right corner, in logical pixels.
    const MARGIN: f64 = 20.0;

    /// Bumped by every show, so only the latest one's timer hides the window.
    static GENERATION: AtomicU64 = AtomicU64::new(0);

    /// Create the (hidden) overlay window. Call once, at startup.
    pub fn create(app: &AppHandle) -> tauri::Result<()> {
        let window = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("overlay.html".into()))
            .title("clip.sh overlay")
            .inner_size(WIDTH, HEIGHT)
            .resizable(false)
            .decorations(false)
            .transparent(true)
            .shadow(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .focusable(false)
            .focused(false)
            .visible(false)
            .build()?;
        window.set_ignore_cursor_events(true)?;
        Ok(())
    }

    /// Show `popup` over the game in the foreground. Nothing happens when
    /// clip.sh itself is in front: the app shows its own toast then.
    pub fn show(app: &AppHandle, popup: Popup) {
        let Some(window) = app.get_webview_window(LABEL) else {
            return;
        };
        let Ok(overlay) = window.hwnd() else { return };
        // SAFETY: plain query.
        let fg = unsafe { GetForegroundWindow() };
        let ours = app
            .webview_windows()
            .values()
            .filter_map(|w| w.hwnd().ok())
            .any(|h| h.0 == fg.0);
        if fg.0.is_null() || ours {
            return;
        }

        // Top-right corner of the game's client area.
        let Some((right, top)) = client_top_right(fg) else {
            return;
        };
        let scale = window.scale_factor().unwrap_or(1.0);
        let width = (WIDTH * scale).round() as i32;
        let margin = (MARGIN * scale).round() as i32;
        let _ = window.set_position(PhysicalPosition::new(right - width - margin, top + margin));

        let ms = popup.ms;
        let _ = app.emit_to(LABEL, SHOW_EVENT, popup);
        let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;

        // Show without activating; Tauri's own show() may take focus.
        let hwnd = overlay.0 as isize;
        let _ = app.run_on_main_thread(move || {
            let hwnd = HWND(hwnd as *mut _);
            // SAFETY: our own live window; no activation, no size or move.
            unsafe {
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
            }
        });

        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_millis(ms)).await;
            if GENERATION.load(Ordering::SeqCst) != generation {
                return; // a newer popup owns the window now
            }
            let _ = app.run_on_main_thread(move || {
                // SAFETY: our own window.
                unsafe {
                    let _ = ShowWindow(HWND(hwnd as *mut _), SW_HIDE);
                }
            });
        });
    }

    /// Screen coordinates of the window's client-area top-right corner.
    fn client_top_right(hwnd: HWND) -> Option<(i32, i32)> {
        let mut r = RECT::default();
        // SAFETY: plain queries on a window handle; a stale one just fails.
        unsafe {
            GetClientRect(hwnd, &mut r).ok()?;
            let mut p = POINT {
                x: r.right,
                y: r.top,
            };
            ClientToScreen(hwnd, &mut p).ok().ok()?;
            Some((p.x, p.y))
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use tauri::AppHandle;

    use super::Popup;

    pub fn create(_app: &AppHandle) -> tauri::Result<()> {
        Ok(())
    }

    pub fn show(_app: &AppHandle, _popup: Popup) {}
}

/// Whether the overlay is turned on.
pub fn enabled(app: &AppHandle) -> bool {
    use tauri::Manager;
    app.try_state::<crate::AppState>()
        .is_some_and(|s| s.settings.lock().unwrap().overlay)
}
