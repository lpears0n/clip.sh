//! Display information from the Wayland compositor.

use anyhow::Result;
use wayland_client::{
    Connection, Dispatch, QueueHandle,
    protocol::{wl_output, wl_registry},
};

#[derive(Default)]
struct State {
    /// Current mode refresh rate per output, in mHz.
    refresh: Vec<i32>,
}

/// Highest current refresh rate across connected displays, in Hz. Capture
/// can't produce more distinct frames per second than this.
pub fn max_refresh_hz() -> Result<Option<f64>> {
    let conn = Connection::connect_to_env()?;
    let mut queue = conn.new_event_queue();
    conn.display().get_registry(&queue.handle(), ());
    let mut state = State::default();
    // Globals, then each output's modes.
    queue.roundtrip(&mut state)?;
    queue.roundtrip(&mut state)?;
    Ok(state
        .refresh
        .iter()
        .max()
        .map(|&mhz| f64::from(mhz) / 1000.0))
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        _: &mut Self,
        reg: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
            && interface == "wl_output"
        {
            reg.bind::<wl_output::WlOutput, _, _>(name, version.min(4), qh, ());
        }
    }
}

impl Dispatch<wl_output::WlOutput, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Mode { flags, refresh, .. } = event
            && let wayland_client::WEnum::Value(f) = flags
            && f.contains(wl_output::Mode::Current)
        {
            state.refresh.push(refresh);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn reports_a_refresh_rate_when_on_wayland() {
        if std::env::var_os("WAYLAND_DISPLAY").is_none() {
            return;
        }
        let hz = super::max_refresh_hz().unwrap().expect("no outputs");
        assert!((20.0..=1000.0).contains(&hz), "{hz}");
        println!("max refresh {hz} Hz");
    }
}
