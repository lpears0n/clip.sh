//! System-wide hotkeys through KDE's KGlobalAccel (D-Bus). Unlike X11 grabs
//! they fire while a Wayland or XWayland game has focus, and unlike the
//! GlobalShortcuts portal they need no confirmation dialog. The bindings also
//! show up in System Settings → Shortcuts, one per [`Action`].

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use tokio::sync::mpsc;
use zbus::{Connection, Proxy, zvariant::OwnedObjectPath};

use crate::hotkey::{Action, Binding};

const SERVICE: &str = "org.kde.kglobalaccel";
const COMPONENT: &str = "clip_sh";
/// KGlobalAccel::SetShortcutFlag: SetPresent | NoAutoloading.
const SET_FLAGS: u32 = 0x2 | 0x4;

/// The registered hotkeys. Presses arrive on the receiver returned by
/// [`GlobalShortcut::register`]; dropping this stops listening.
pub struct GlobalShortcut {
    conn: Connection,
    task: tokio::task::JoinHandle<()>,
}

impl GlobalShortcut {
    /// Register every action and bind each to its keys (e.g.
    /// `"Ctrl+Shift+F8"`); empty keys register the action unbound.
    pub async fn register(bindings: &[Binding]) -> Result<(Self, mpsc::UnboundedReceiver<Action>)> {
        let conn = Connection::session().await?;
        let accel = accel(&conn).await?;
        for b in bindings {
            accel
                .call_method("doRegister", &(action_id(b.action),))
                .await
                .context("KGlobalAccel unavailable (not KDE?)")?;
            // A taken key leaves this one action unbound (the user can pick
            // another in Settings) rather than losing every hotkey.
            if let Err(e) = set_keys(&conn, b.action, &b.keys).await {
                tracing::warn!("hotkey for {}: {e:#}", b.action.id());
            }
        }

        let path: OwnedObjectPath = accel
            .call("getComponent", &(COMPONENT,))
            .await
            .context("KGlobalAccel component missing")?;
        let component = Proxy::new(&conn, SERVICE, path, "org.kde.kglobalaccel.Component").await?;
        let mut presses = component.receive_signal("globalShortcutPressed").await?;
        let (tx, rx) = mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            while let Some(msg) = presses.next().await {
                let Ok((_, action, _)) = msg.body().deserialize::<(String, String, i64)>() else {
                    continue;
                };
                if let Some(action) = Action::from_id(&action)
                    && tx.send(action).is_err()
                {
                    break;
                }
            }
        });
        Ok((Self { conn, task }, rx))
    }

    /// Rebind one action (empty `keys` unbinds it). Fails if the combination
    /// can't be parsed or another application already owns it.
    pub async fn set_keys(&self, action: Action, keys: &str) -> Result<()> {
        set_keys(&self.conn, action, keys).await
    }
}

async fn set_keys(conn: &Connection, action: Action, keys: &str) -> Result<()> {
    let accel = accel(conn).await?;
    if keys.trim().is_empty() {
        let _: Vec<(Vec<i32>,)> = accel
            .call(
                "setShortcutKeys",
                &(action_id(action), Vec::<(Vec<i32>,)>::new(), SET_FLAGS),
            )
            .await?;
        return Ok(());
    }
    let code = parse(keys)?;
    let set: Vec<(Vec<i32>,)> = accel
        .call(
            "setShortcutKeys",
            &(action_id(action), vec![(vec![code],)], SET_FLAGS),
        )
        .await?;
    if set.first().and_then(|(k,)| k.first()) != Some(&code) {
        bail!("{keys} is already used by another shortcut");
    }
    Ok(())
}

impl Drop for GlobalShortcut {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn accel(conn: &Connection) -> Result<Proxy<'static>> {
    Ok(Proxy::new(conn, SERVICE, "/kglobalaccel", "org.kde.KGlobalAccel").await?)
}

/// [componentUnique, actionUnique, componentFriendly, actionFriendly]
fn action_id(action: Action) -> Vec<&'static str> {
    vec![COMPONENT, action.id(), "clip.sh", action.description()]
}

/// Parse `"Ctrl+Shift+F8"` into a Qt key combination (`Qt::Key | modifiers`).
/// Modifier names: Ctrl, Shift, Alt, Meta. Keys: A–Z, 0–9, F1–F24 and the
/// named keys below.
pub fn parse(keys: &str) -> Result<i32> {
    let mut code: i32 = 0;
    let mut key = None;
    for part in keys.split('+').map(str::trim).filter(|p| !p.is_empty()) {
        match part.to_ascii_lowercase().as_str() {
            "shift" => code |= 0x0200_0000,
            "ctrl" | "control" => code |= 0x0400_0000,
            "alt" => code |= 0x0800_0000,
            "meta" | "super" => code |= 0x1000_0000,
            _ => {
                if key.replace(qt_key(part)?).is_some() {
                    bail!("{keys:?} has more than one non-modifier key");
                }
            }
        }
    }
    let key = key.with_context(|| format!("{keys:?} has no key besides modifiers"))?;
    Ok(code | key)
}

fn qt_key(name: &str) -> Result<i32> {
    let upper = name.to_ascii_uppercase();
    if upper.len() == 1 {
        let c = upper.as_bytes()[0];
        if c.is_ascii_alphanumeric() {
            return Ok(c as i32);
        }
    }
    if let Some(n) = upper.strip_prefix('F').and_then(|n| n.parse::<i32>().ok())
        && (1..=24).contains(&n)
    {
        return Ok(0x0100_0030 + n - 1);
    }
    Ok(match upper.as_str() {
        "SPACE" => 0x20,
        "ESCAPE" | "ESC" => 0x0100_0000,
        "TAB" => 0x0100_0001,
        "BACKSPACE" => 0x0100_0003,
        "ENTER" | "RETURN" => 0x0100_0004,
        "INSERT" => 0x0100_0006,
        "DELETE" => 0x0100_0007,
        "PAUSE" => 0x0100_0008,
        "PRINT" | "PRINTSCREEN" => 0x0100_0009,
        "HOME" => 0x0100_0010,
        "END" => 0x0100_0011,
        "LEFT" => 0x0100_0012,
        "UP" => 0x0100_0013,
        "RIGHT" => 0x0100_0014,
        "DOWN" => 0x0100_0015,
        "PAGEUP" => 0x0100_0016,
        "PAGEDOWN" => 0x0100_0017,
        "SCROLLLOCK" => 0x0100_0026,
        _ => bail!("unsupported key {name:?}"),
    })
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn parses_combinations() {
        assert_eq!(parse("F8").unwrap(), 0x0100_0037);
        assert_eq!(
            parse("Ctrl+Shift+S").unwrap(),
            0x0400_0000 | 0x0200_0000 | 0x53
        );
        assert_eq!(parse("alt + f10").unwrap(), 0x0800_0000 | 0x0100_0039);
        assert_eq!(parse("Meta+PageDown").unwrap(), 0x1000_0000 | 0x0100_0017);
        assert!(parse("Ctrl+Shift").is_err());
        assert!(parse("A+B").is_err());
        assert!(parse("Ctrl+Nope").is_err());
    }
}
