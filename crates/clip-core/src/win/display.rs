//! Display refresh rates through the GDI display-settings API.

use anyhow::Result;
use windows::{
    Win32::Graphics::Gdi::{
        DEVMODEW, DISPLAY_DEVICEW, ENUM_CURRENT_SETTINGS, EnumDisplayDevicesW, EnumDisplaySettingsW,
    },
    core::PCWSTR,
};

/// Highest current refresh rate across connected displays, in Hz.
pub fn max_refresh_hz() -> Result<Option<f64>> {
    let mut best: Option<u32> = None;
    let mut i = 0u32;
    loop {
        let mut dev = DISPLAY_DEVICEW {
            cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32,
            ..Default::default()
        };
        // SAFETY: `dev` is a correctly sized, writable DISPLAY_DEVICEW.
        if !unsafe { EnumDisplayDevicesW(PCWSTR::null(), i, &mut dev, 0) }.as_bool() {
            break;
        }
        i += 1;
        let mut mode = DEVMODEW {
            dmSize: std::mem::size_of::<DEVMODEW>() as u16,
            ..Default::default()
        };
        // SAFETY: `dev.DeviceName` is NUL-terminated; `mode` is sized.
        let ok = unsafe {
            EnumDisplaySettingsW(
                PCWSTR(dev.DeviceName.as_ptr()),
                ENUM_CURRENT_SETTINGS,
                &mut mode,
            )
        }
        .as_bool();
        if ok && mode.dmDisplayFrequency > 1 {
            best = Some(best.map_or(mode.dmDisplayFrequency, |b| b.max(mode.dmDisplayFrequency)));
        }
    }
    Ok(best.map(f64::from))
}
