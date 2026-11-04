//! Windows: window capture (Windows Graphics Capture) through GStreamer's
//! D3D11 elements,
//! game detection by walking top-level windows, and display queries.

pub mod auto;
pub mod display;
pub mod game;
pub mod hdr_window;

/// What the recorder captures: the game's window, by `HWND`; with
/// `hdr_monitor` set, that window's area of its HDR monitor, tone-mapped
/// (HDR compatibility); with `hdr_window` set, the window itself in HDR,
/// tone-mapped for that display (experimental).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Target {
    pub window: isize,
    pub cursor: bool,
    pub hdr_monitor: Option<isize>,
    pub hdr_window: Option<HdrWindow>,
}

/// The HDR display a window is tone-mapped for, and the GPU it is on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HdrWindow {
    pub display: crate::HdrDisplay,
    pub adapter_luid: Option<i64>,
}
