//! Windows: Windows Graphics Capture through GStreamer's D3D11 elements,
//! game detection by walking top-level windows, and display queries.

pub mod auto;
pub mod display;
pub mod game;

/// What the recorder captures: the game's monitor (`HMONITOR`) or its
/// window (`HWND`), as `method` says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    pub monitor: isize,
    pub window: isize,
    pub method: crate::capture::CaptureMethod,
    pub cursor: bool,
}
