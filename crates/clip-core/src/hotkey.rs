//! Global hotkeys, on whatever the desktop offers:
//! 1. KGlobalAccel (KDE): no dialog, keys set by us.
//! 2. The GlobalShortcuts portal (GNOME, Hyprland, other portal backends):
//!    the desktop confirms the bindings once and lets the user change them.
//!
//! Where neither exists, the user binds `clipsh save` (and friends) in their
//! desktop's keyboard settings instead. Windows registers its hotkeys in the
//! app through the OS; only [`Action`] and [`Binding`] are shared with it.

use serde::{Deserialize, Serialize};

/// What a hotkey does. Each action is one entry in the desktop's shortcut
/// settings and one line of `clipsh`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    /// Save the replay buffer as a clip.
    SaveClip,
    /// Save the newest frame as a PNG.
    Screenshot,
}

impl Action {
    pub const ALL: [Action; 2] = [Action::SaveClip, Action::Screenshot];

    /// Stable identifier, as the desktop sees it.
    pub fn id(self) -> &'static str {
        match self {
            Action::SaveClip => "save-clip",
            Action::Screenshot => "screenshot",
        }
    }

    /// Label shown in the desktop's shortcut settings.
    pub fn description(self) -> &'static str {
        match self {
            Action::SaveClip => "Save clip",
            Action::Screenshot => "Save screenshot",
        }
    }

    pub fn from_id(id: &str) -> Option<Action> {
        Action::ALL.into_iter().find(|a| a.id() == id)
    }
}

/// An action and the keys for it. Empty `keys` registers the action without
/// binding it (the user can still assign keys in the desktop's settings).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub action: Action,
    pub keys: String,
}

impl Binding {
    pub fn new(action: Action, keys: impl Into<String>) -> Self {
        Self {
            action,
            keys: keys.into(),
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux::{Hotkey, HotkeyBackend, xdg_trigger};

#[cfg(target_os = "linux")]
mod linux {
    use anyhow::{Context, Result, bail};
    use ashpd::desktop::{
        Session,
        global_shortcuts::{GlobalShortcuts, NewShortcut},
    };
    use futures_util::StreamExt;
    use serde::Serialize;
    use tokio::sync::mpsc;

    use super::{Action, Binding};
    use crate::kwin::shortcut::{self, GlobalShortcut};

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
    #[serde(rename_all = "lowercase")]
    pub enum HotkeyBackend {
        Kglobalaccel,
        Portal,
    }

    pub struct Hotkey {
        inner: Inner,
    }

    enum Inner {
        Kde(GlobalShortcut),
        Portal {
            proxy: GlobalShortcuts,
            session: Session<GlobalShortcuts>,
            task: tokio::task::JoinHandle<()>,
        },
    }

    impl Hotkey {
        /// Register every binding (keys like `"F8"` or `"Ctrl+Shift+S"`).
        /// Presses arrive on the returned receiver as the action pressed.
        /// `skip_kde` forces the portal (for testing on KDE).
        pub async fn register(
            bindings: &[Binding],
            skip_kde: bool,
        ) -> Result<(Self, mpsc::UnboundedReceiver<Action>)> {
            if !skip_kde {
                match GlobalShortcut::register(bindings).await {
                    Ok((s, rx)) => {
                        return Ok((
                            Self {
                                inner: Inner::Kde(s),
                            },
                            rx,
                        ));
                    }
                    Err(e) => {
                        tracing::info!("KGlobalAccel unavailable ({e:#}); trying the portal")
                    }
                }
            }
            let (inner, rx) = bind_portal(bindings).await?;
            Ok((Self { inner }, rx))
        }

        pub fn backend(&self) -> HotkeyBackend {
            match self.inner {
                Inner::Kde(_) => HotkeyBackend::Kglobalaccel,
                Inner::Portal { .. } => HotkeyBackend::Portal,
            }
        }

        /// Change one action's keys (empty unbinds it). With the portal the
        /// desktop owns the bindings, so this opens its shortcut settings
        /// (portal v2) instead.
        pub async fn set_keys(&self, action: Action, keys: &str) -> Result<()> {
            match &self.inner {
                Inner::Kde(s) => s.set_keys(action, keys).await,
                Inner::Portal { proxy, session, .. } => {
                    if proxy.version() < 2 {
                        bail!("change the shortcut in your desktop's keyboard settings");
                    }
                    proxy
                        .configure_shortcuts(session, None, Default::default())
                        .await
                        .context("opening the desktop's shortcut settings")
                }
            }
        }
    }

    impl Drop for Hotkey {
        fn drop(&mut self) {
            if let Inner::Portal { task, .. } = &self.inner {
                task.abort();
            }
        }
    }

    async fn bind_portal(bindings: &[Binding]) -> Result<(Inner, mpsc::UnboundedReceiver<Action>)> {
        let proxy = GlobalShortcuts::new()
            .await
            .context("no global shortcuts portal; bind `clipsh save` in your desktop settings")?;
        let session = proxy.create_session(Default::default()).await?;
        let mut shortcuts = Vec::with_capacity(bindings.len());
        for b in bindings {
            let mut s = NewShortcut::new(b.action.id(), b.action.description());
            if !b.keys.is_empty() {
                s = s.preferred_trigger(xdg_trigger(&b.keys)?.as_str());
            }
            shortcuts.push(s);
        }
        let bound = proxy
            .bind_shortcuts(&session, &shortcuts, None, Default::default())
            .await?
            .response()
            .context("the desktop declined the shortcuts")?;
        for s in bound.shortcuts() {
            tracing::info!(
                action = s.id(),
                trigger = s.trigger_description(),
                "hotkey bound through the portal"
            );
        }

        let mut activated = proxy.receive_activated().await?;
        let (tx, rx) = mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            while let Some(a) = activated.next().await {
                if let Some(action) = Action::from_id(a.shortcut_id())
                    && tx.send(action).is_err()
                {
                    break;
                }
            }
        });
        Ok((
            Inner::Portal {
                proxy,
                session,
                task,
            },
            rx,
        ))
    }

    /// Our key syntax ("Ctrl+Shift+S", "Meta+F8") in the XDG shortcuts spec's
    /// form ("CTRL+SHIFT+s", "LOGO+F8"): uppercase modifiers, xkb keysym names.
    pub fn xdg_trigger(keys: &str) -> Result<String> {
        // Reuse the KDE parser for validation so both paths accept the same input.
        shortcut::parse(keys)?;
        let parts: Vec<String> = keys
            .split('+')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(|p| match p.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => "CTRL".to_owned(),
                "shift" => "SHIFT".to_owned(),
                "alt" => "ALT".to_owned(),
                "meta" | "super" => "LOGO".to_owned(),
                key => keysym(key),
            })
            .collect();
        Ok(parts.join("+"))
    }

    /// xkb keysym name for one of our key names.
    fn keysym(key: &str) -> String {
        match key {
            k if k.len() == 1 => k.to_owned(),
            k if k.starts_with('f') && k[1..].parse::<u8>().is_ok() => k.to_uppercase(),
            "space" => "space".into(),
            "escape" | "esc" => "Escape".into(),
            "tab" => "Tab".into(),
            "backspace" => "BackSpace".into(),
            "enter" | "return" => "Return".into(),
            "insert" => "Insert".into(),
            "delete" => "Delete".into(),
            "pause" => "Pause".into(),
            "print" | "printscreen" => "Print".into(),
            "home" => "Home".into(),
            "end" => "End".into(),
            "left" => "Left".into(),
            "up" => "Up".into(),
            "right" => "Right".into(),
            "down" => "Down".into(),
            "pageup" => "Prior".into(),
            "pagedown" => "Next".into(),
            "scrolllock" => "Scroll_Lock".into(),
            other => other.to_owned(),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::xdg_trigger;

        #[test]
        fn converts_to_xdg_triggers() {
            assert_eq!(xdg_trigger("F8").unwrap(), "F8");
            assert_eq!(xdg_trigger("Ctrl+Shift+S").unwrap(), "CTRL+SHIFT+s");
            assert_eq!(xdg_trigger("Meta+PageDown").unwrap(), "LOGO+Next");
            assert_eq!(xdg_trigger("Alt+Print").unwrap(), "ALT+Print");
            assert!(xdg_trigger("Ctrl+Nope").is_err());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Action;

    #[test]
    fn action_ids_round_trip() {
        for a in Action::ALL {
            assert_eq!(Action::from_id(a.id()), Some(a));
        }
        assert_eq!(Action::from_id("nope"), None);
        assert_eq!(
            serde_json::to_string(&Action::SaveClip).unwrap(),
            "\"save-clip\""
        );
    }
}
