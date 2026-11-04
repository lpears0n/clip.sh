//! Windows: window capture (Windows Graphics Capture) through GStreamer's
//! D3D11 elements,
//! game detection by walking top-level windows, and display queries.

pub mod auto;
pub mod display;
pub mod game;

/// What the recorder captures: the game's window, by `HWND`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    pub window: isize,
    pub cursor: bool,
}
