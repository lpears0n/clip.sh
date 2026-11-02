//! Which capture backend the automatic recorder is using.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    /// KDE Plasma: each game window is streamed directly, no picker.
    Kwin,
    /// Any Wayland desktop: the screen chosen once through the portal.
    Portal,
    /// Windows Graphics Capture of the monitor the game is on.
    Windows,
}
