//! The global keys: save a clip, save a screenshot. Linux asks the desktop
//! (KGlobalAccel or the GlobalShortcuts portal); Windows registers native
//! hotkeys through the global-shortcut plugin, which fire over fullscreen
//! games.

use clip_core::Action;
use serde::Serialize;
use tauri::{AppHandle, Manager};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
#[allow(dead_code)] // each platform constructs its own variants
pub enum Backend {
    Kglobalaccel,
    Portal,
    /// The OS's own hotkey registration (Windows).
    Native,
}

/// Run `action` against the recorder, logging a failure (a hotkey press has
/// no caller to report to; the UI hears about the result as an event).
pub async fn fire(app: &AppHandle, action: Action) {
    let state = app.state::<crate::AppState>();
    let result = match action {
        Action::SaveClip => state.service.save(state.clips_dir()).await,
        Action::Screenshot => state.service.screenshot(state.clips_dir()).await,
    };
    if let Err(e) = result {
        tracing::warn!("hotkey {}: {e}", action.id());
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::Backend;
    use clip_core::{
        Action, Binding,
        hotkey::{Hotkey, HotkeyBackend},
    };
    use tauri::AppHandle;

    #[derive(Default)]
    pub struct Hotkeys {
        /// `None` until registered, and when no hotkey service exists.
        inner: tokio::sync::Mutex<Option<Hotkey>>,
    }

    impl Hotkeys {
        /// Register every binding and act on presses for the life of the
        /// app. Logs and gives up if the desktop offers no service.
        pub async fn start(&self, app: AppHandle, bindings: Vec<Binding>) {
            // CLIPSH_HOTKEY_PORTAL=1 tests the portal path on KDE.
            let skip_kde = std::env::var_os("CLIPSH_HOTKEY_PORTAL").is_some();
            let (shortcut, mut presses) = match Hotkey::register(&bindings, skip_kde).await {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!("global hotkeys unavailable: {e:#}");
                    return;
                }
            };
            *self.inner.lock().await = Some(shortcut);
            while let Some(action) = presses.recv().await {
                super::fire(&app, action).await;
            }
        }

        pub async fn backend(&self) -> Option<Backend> {
            self.inner.lock().await.as_ref().map(|h| match h.backend() {
                HotkeyBackend::Kglobalaccel => Backend::Kglobalaccel,
                HotkeyBackend::Portal => Backend::Portal,
            })
        }

        pub async fn set_keys(
            &self,
            _app: &AppHandle,
            action: Action,
            keys: &str,
        ) -> Result<(), String> {
            match self.inner.lock().await.as_ref() {
                Some(s) => s.set_keys(action, keys).await.map_err(|e| format!("{e:#}")),
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
    use std::{collections::HashMap, sync::Mutex};

    use super::Backend;
    use clip_core::{Action, Binding};
    use tauri::{AppHandle, Manager};
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

    #[derive(Default)]
    pub struct Hotkeys {
        /// What each registered shortcut does. A std mutex: the plugin's
        /// handler is synchronous.
        registered: Mutex<HashMap<Shortcut, Action>>,
    }

    /// The plugin; its handler runs the action bound to the pressed key.
    /// Tied to the Wry runtime: the handler hands the app to [`super::fire`],
    /// which takes the app's concrete handle.
    pub fn plugin() -> tauri::plugin::TauriPlugin<tauri::Wry> {
        tauri_plugin_global_shortcut::Builder::<tauri::Wry>::new()
            .with_handler(|app, shortcut, event| {
                if event.state() != ShortcutState::Pressed {
                    return;
                }
                let action = app
                    .state::<crate::AppState>()
                    .hotkeys
                    .registered
                    .lock()
                    .unwrap()
                    .get(shortcut)
                    .copied();
                let Some(action) = action else { return };
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    super::fire(&app, action).await;
                });
            })
            .build()
    }

    impl Hotkeys {
        pub async fn start(&self, app: AppHandle, bindings: Vec<Binding>) {
            for b in bindings {
                if let Err(e) = self.set_keys(&app, b.action, &b.keys).await {
                    tracing::warn!("global hotkey for {}: {e}", b.action.id());
                }
            }
        }

        pub async fn backend(&self) -> Option<Backend> {
            (!self.registered.lock().unwrap().is_empty()).then_some(Backend::Native)
        }

        /// Bind `action` to `keys` (empty unbinds it).
        pub async fn set_keys(
            &self,
            app: &AppHandle,
            action: Action,
            keys: &str,
        ) -> Result<(), String> {
            let gs = app.global_shortcut();
            let mut current = self.registered.lock().unwrap();
            let old: Vec<Shortcut> = current
                .iter()
                .filter(|(_, a)| **a == action)
                .map(|(s, _)| *s)
                .collect();
            for s in old {
                let _ = gs.unregister(s);
                current.remove(&s);
            }
            if keys.trim().is_empty() {
                return Ok(());
            }
            let shortcut: Shortcut = super::to_plugin_syntax(keys)
                .parse()
                .map_err(|e| format!("can't use {keys}: {e}"))?;
            if current.contains_key(&shortcut) {
                return Err(format!("{keys} is already used by another clip.sh hotkey"));
            }
            gs.register(shortcut)
                .map_err(|e| format!("Windows refused {keys}: {e} (another app may own it)"))?;
            current.insert(shortcut, action);
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
