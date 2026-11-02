//! The global "save clip" key. Linux asks the desktop (KGlobalAccel or the
//! GlobalShortcuts portal); Windows registers a native hotkey through the
//! global-shortcut plugin, which fires over fullscreen games.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
#[allow(dead_code)] // each platform constructs its own variants
pub enum Backend {
    Kglobalaccel,
    Portal,
    /// The OS's own hotkey registration (Windows).
    Native,
}

#[cfg(target_os = "linux")]
mod imp {
    use super::Backend;
    use clip_core::hotkey::{Hotkey, HotkeyBackend};
    use tauri::{AppHandle, Manager};

    #[derive(Default)]
    pub struct Hotkeys {
        /// `None` until registered, and when no hotkey service exists.
        inner: tokio::sync::Mutex<Option<Hotkey>>,
    }

    impl Hotkeys {
        /// Register `keys` and save a clip on every press, for the life of
        /// the app. Logs and gives up if the desktop offers no service.
        pub async fn start(&self, app: AppHandle, keys: String) {
            // CLIPSH_HOTKEY_PORTAL=1 tests the portal path on KDE.
            let skip_kde = std::env::var_os("CLIPSH_HOTKEY_PORTAL").is_some();
            let (shortcut, mut presses) = match Hotkey::register(&keys, skip_kde).await {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!("global hotkey unavailable: {e:#}");
                    return;
                }
            };
            *self.inner.lock().await = Some(shortcut);
            let state = app.state::<crate::AppState>();
            while presses.recv().await.is_some() {
                if let Err(e) = state.service.save(state.clips_dir()).await {
                    tracing::warn!("hotkey save failed: {e}");
                }
            }
        }

        pub async fn backend(&self) -> Option<Backend> {
            self.inner.lock().await.as_ref().map(|h| match h.backend() {
                HotkeyBackend::Kglobalaccel => Backend::Kglobalaccel,
                HotkeyBackend::Portal => Backend::Portal,
            })
        }

        pub async fn set_keys(&self, _app: &AppHandle, keys: &str) -> Result<(), String> {
            match self.inner.lock().await.as_ref() {
                Some(s) => s.set_keys(keys).await.map_err(|e| format!("{e:#}")),
                None => Err(
                    "your desktop has no global shortcut service; bind `clipsh save` in its keyboard settings"
                        .into(),
                ),
            }
        }
    }
}

#[cfg(windows)]
mod imp {
    use super::Backend;
    use tauri::{AppHandle, Manager};
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

    #[derive(Default)]
    pub struct Hotkeys {
        registered: tokio::sync::Mutex<Option<Shortcut>>,
    }

    /// The plugin; its handler saves a clip on every press of our shortcut.
    pub fn plugin<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
        tauri_plugin_global_shortcut::Builder::new()
            .with_handler(|app, _shortcut, event| {
                if event.state() != ShortcutState::Pressed {
                    return;
                }
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    let state = app.state::<crate::AppState>();
                    if let Err(e) = state.service.save(state.clips_dir()).await {
                        tracing::warn!("hotkey save failed: {e}");
                    }
                });
            })
            .build()
    }

    impl Hotkeys {
        pub async fn start(&self, app: AppHandle, keys: String) {
            if let Err(e) = self.set_keys(&app, &keys).await {
                tracing::warn!("global hotkey unavailable: {e}");
            }
        }

        pub async fn backend(&self) -> Option<Backend> {
            self.registered
                .lock()
                .await
                .as_ref()
                .map(|_| Backend::Native)
        }

        pub async fn set_keys(&self, app: &AppHandle, keys: &str) -> Result<(), String> {
            let shortcut: Shortcut = super::to_plugin_syntax(keys)
                .parse()
                .map_err(|e| format!("can't use {keys}: {e}"))?;
            let gs = app.global_shortcut();
            let mut current = self.registered.lock().await;
            if let Some(old) = current.take() {
                let _ = gs.unregister(old);
            }
            gs.register(shortcut)
                .map_err(|e| format!("Windows refused {keys}: {e} (another app may own it)"))?;
            *current = Some(shortcut);
            Ok(())
        }
    }
}

pub use imp::Hotkeys;
#[cfg(windows)]
pub use imp::plugin;

/// Our settings syntax (`Ctrl+Shift+F8`, keys named as `hotkeyFromEvent`
/// in the UI emits them) to the global-shortcut plugin's (`Control+Shift+F8`,
/// `KeyA`, `Digit1`, `ArrowUp`).
#[allow(dead_code)]
pub fn to_plugin_syntax(keys: &str) -> String {
    keys.split('+')
        .map(|k| match k {
            "Ctrl" => "Control".to_string(),
            "Meta" => "Super".to_string(),
            "Print" => "PrintScreen".to_string(),
            "Up" => "ArrowUp".to_string(),
            "Down" => "ArrowDown".to_string(),
            "Left" => "ArrowLeft".to_string(),
            "Right" => "ArrowRight".to_string(),
            k if k.len() == 1 && k.chars().all(|c| c.is_ascii_uppercase()) => format!("Key{k}"),
            k if k.len() == 1 && k.chars().all(|c| c.is_ascii_digit()) => format!("Digit{k}"),
            k => k.to_string(),
        })
        .collect::<Vec<_>>()
        .join("+")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_key_names() {
        assert_eq!(to_plugin_syntax("F8"), "F8");
        assert_eq!(to_plugin_syntax("Ctrl+Shift+S"), "Control+Shift+KeyS");
        assert_eq!(to_plugin_syntax("Alt+1"), "Alt+Digit1");
        assert_eq!(to_plugin_syntax("Meta+Up"), "Super+ArrowUp");
        assert_eq!(to_plugin_syntax("Print"), "PrintScreen");
    }
}
