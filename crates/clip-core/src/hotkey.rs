//! The "save clip" hotkey, on whatever the desktop offers:
//! 1. KGlobalAccel (KDE): no dialog, key set by us.
//! 2. The GlobalShortcuts portal (GNOME, Hyprland, other portal backends):
//!    the desktop confirms the binding once and lets the user change it.
//!
//! Where neither exists, the user binds `clipsh save` in their desktop's
//! keyboard settings instead.

use anyhow::{Context, Result, bail};
use ashpd::desktop::{
    Session,
    global_shortcuts::{GlobalShortcuts, NewShortcut},
};
use futures_util::StreamExt;
use serde::Serialize;
use tokio::sync::mpsc;

use crate::kwin::shortcut::{self, GlobalShortcut};

const ID: &str = "save-clip";
const DESCRIPTION: &str = "Save clip";

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
    /// Register `keys` (e.g. `"F8"`, `"Ctrl+Shift+S"`). Presses arrive on the
    /// returned receiver. `skip_kde` forces the portal (for testing on KDE).
    pub async fn register(
        keys: &str,
        skip_kde: bool,
    ) -> Result<(Self, mpsc::UnboundedReceiver<()>)> {
        if !skip_kde {
            match GlobalShortcut::register(keys).await {
                Ok((s, rx)) => {
                    return Ok((
                        Self {
                            inner: Inner::Kde(s),
                        },
                        rx,
                    ));
                }
                Err(e) => tracing::info!("KGlobalAccel unavailable ({e:#}); trying the portal"),
            }
        }
        let (inner, rx) = bind_portal(keys).await?;
        Ok((Self { inner }, rx))
    }

    pub fn backend(&self) -> HotkeyBackend {
        match self.inner {
            Inner::Kde(_) => HotkeyBackend::Kglobalaccel,
            Inner::Portal { .. } => HotkeyBackend::Portal,
        }
    }

    /// Change the key. With the portal the desktop owns the binding, so this
    /// opens its shortcut settings (portal v2) instead.
    pub async fn set_keys(&self, keys: &str) -> Result<()> {
        match &self.inner {
            Inner::Kde(s) => s.set_keys(keys).await,
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

async fn bind_portal(keys: &str) -> Result<(Inner, mpsc::UnboundedReceiver<()>)> {
    let proxy = GlobalShortcuts::new()
        .await
        .context("no global shortcuts portal; bind `clipsh save` in your desktop settings")?;
    let session = proxy.create_session(Default::default()).await?;
    let trigger = xdg_trigger(keys)?;
    let shortcut = NewShortcut::new(ID, DESCRIPTION).preferred_trigger(trigger.as_str());
    let bound = proxy
        .bind_shortcuts(&session, &[shortcut], None, Default::default())
        .await?
        .response()
        .context("the desktop declined the shortcut")?;
    if let Some(s) = bound.shortcuts().iter().find(|s| s.id() == ID) {
        tracing::info!(
            trigger = s.trigger_description(),
            "hotkey bound through the portal"
        );
    }

    let mut activated = proxy.receive_activated().await?;
    let (tx, rx) = mpsc::unbounded_channel();
    let task = tokio::spawn(async move {
        while let Some(a) = activated.next().await {
            if a.shortcut_id() == ID && tx.send(()).is_err() {
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
