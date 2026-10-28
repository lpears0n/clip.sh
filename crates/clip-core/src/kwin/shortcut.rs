//! System-wide hotkey through KDE's KGlobalAccel (D-Bus). Unlike X11 grabs it
//! fires while a Wayland or XWayland game has focus, and unlike the
//! GlobalShortcuts portal it needs no confirmation dialog. The binding also
//! shows up in System Settings → Shortcuts.

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use tokio::sync::mpsc;
use zbus::{Connection, Proxy, zvariant::OwnedObjectPath};

const SERVICE: &str = "org.kde.kglobalaccel";
const COMPONENT: &str = "clip_sh";
const ACTION: &str = "save-clip";
/// KGlobalAccel::SetShortcutFlag: SetPresent | NoAutoloading.
const SET_FLAGS: u32 = 0x2 | 0x4;

/// A registered "save clip" hotkey. Presses arrive on the receiver returned by
/// [`GlobalShortcut::register`]; dropping this stops listening.
pub struct GlobalShortcut {
    conn: Connection,
    task: tokio::task::JoinHandle<()>,
}

impl GlobalShortcut {
    /// Register the action and bind it to `keys` (e.g. `"Ctrl+Shift+F8"`).
    pub async fn register(keys: &str) -> Result<(Self, mpsc::UnboundedReceiver<()>)> {
        let conn = Connection::session().await?;
        let accel = accel(&conn).await?;
        accel
            .call_method("doRegister", &(action_id(),))
            .await
            .context("KGlobalAccel unavailable (not KDE?)")?;
        set_keys(&conn, keys).await?;

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
                if action == ACTION && tx.send(()).is_err() {
                    break;
                }
            }
        });
        Ok((Self { conn, task }, rx))
    }

    /// Rebind the hotkey. Fails if the combination can't be parsed or another
    /// application already owns it.
    pub async fn set_keys(&self, keys: &str) -> Result<()> {
        set_keys(&self.conn, keys).await
    }
}

async fn set_keys(conn: &Connection, keys: &str) -> Result<()> {
    {
        let code = parse(keys)?;
        let accel = accel(conn).await?;
        let set: Vec<(Vec<i32>,)> = accel
            .call(
                "setShortcutKeys",
                &(action_id(), vec![(vec![code],)], SET_FLAGS),
            )
            .await?;
        if set.first().and_then(|(k,)| k.first()) != Some(&code) {
            bail!("{keys} is already used by another shortcut");
        }
        Ok(())
    }
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
fn action_id() -> Vec<&'static str> {
    vec![COMPONENT, ACTION, "clip.sh", "Save clip"]
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
