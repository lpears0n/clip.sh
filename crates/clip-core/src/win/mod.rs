//! Windows: window capture (Windows Graphics Capture) through GStreamer's
//! D3D11 elements,
//! game detection by walking top-level windows, and display queries.

pub mod auto;
pub mod display;
pub mod game;

/// What the recorder captures: the game's window, by `HWND`, or with
/// `hdr_monitor` set, that window's area of its HDR monitor, tone-mapped
/// (HDR compatibility).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    pub window: isize,
    pub cursor: bool,
    pub hdr_monitor: Option<isize>,
}
