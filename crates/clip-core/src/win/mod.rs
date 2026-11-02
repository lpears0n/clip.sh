//! Windows: Windows Graphics Capture through GStreamer's D3D11 elements,
//! game detection by walking top-level windows, and display queries.

pub mod auto;
pub mod display;
pub mod game;

/// What the recorder captures: a monitor, by `HMONITOR`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    pub monitor: isize,
    pub cursor: bool,
}
