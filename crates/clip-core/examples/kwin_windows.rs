//! KWin spike: list windows, or screencast one without any picker.
//!
//!     cargo run -p clip-core --example kwin_windows            # list
//!     cargo run -p clip-core --example kwin_windows -- <app_id substring>
//!     CLIPSH_SHOT=out.png cargo run -p clip-core --example kwin_windows -- <app_id>
//!
//! Needs a .desktop file granting the restricted protocols; see
//! `packaging/clip-sh-dev.desktop`.

use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use clip_core::kwin::protocols::{
    screencast::{zkde_screencast_stream_unstable_v1 as stream, zkde_screencast_unstable_v1 as sc},
    window_management::{org_kde_plasma_window as win, org_kde_plasma_window_management as wm},
};
use gstreamer as gst;
use gstreamer::prelude::*;
use wayland_client::{Connection, Dispatch, QueueHandle, protocol::wl_registry};

#[derive(Default, Debug)]
struct Win {
    title: String,
    app_id: String,
    pid: u32,
    state: u32,
}

#[derive(Default)]
struct App {
    wm: Option<wm::OrgKdePlasmaWindowManagement>,
    sc: Option<sc::ZkdeScreencastUnstableV1>,
    windows: HashMap<String, Win>,
    serial: Option<u64>,
    failed: Option<String>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for App {
    fn event(
        app: &mut Self,
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
        {
            match interface.as_str() {
                "org_kde_plasma_window_management" => {
                    app.wm = Some(reg.bind(name, version.min(16), qh, ()));
                }
                "zkde_screencast_unstable_v1" => {
                    app.sc = Some(reg.bind(name, version.min(6), qh, ()));
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<wm::OrgKdePlasmaWindowManagement, ()> for App {
    fn event(
        app: &mut Self,
        wm: &wm::OrgKdePlasmaWindowManagement,
        event: wm::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wm::Event::WindowWithUuid { uuid, .. } = event {
            wm.get_window_by_uuid(uuid.clone(), qh, uuid.clone());
            app.windows.insert(uuid, Win::default());
        }
    }
}

impl Dispatch<win::OrgKdePlasmaWindow, String> for App {
    fn event(
        app: &mut Self,
        _: &win::OrgKdePlasmaWindow,
        event: win::Event,
        uuid: &String,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let win::Event::Unmapped = event {
            app.windows.remove(uuid);
            return;
        }
        let Some(w) = app.windows.get_mut(uuid) else {
            return;
        };
        match event {
            win::Event::TitleChanged { title } => w.title = title,
            win::Event::AppIdChanged { app_id } => w.app_id = app_id,
            win::Event::PidChanged { pid } => w.pid = pid,
            win::Event::StateChanged { flags } => w.state = flags,
            _ => {}
        }
    }
}

impl Dispatch<sc::ZkdeScreencastUnstableV1, ()> for App {
    fn event(
        _: &mut Self,
        _: &sc::ZkdeScreencastUnstableV1,
        _: sc::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<stream::ZkdeScreencastStreamUnstableV1, ()> for App {
    fn event(
        app: &mut Self,
        _: &stream::ZkdeScreencastStreamUnstableV1,
        event: stream::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            stream::Event::Serial {
                object_serial_hi,
                object_serial_low,
            } => {
                app.serial = Some(((object_serial_hi as u64) << 32) | object_serial_low as u64);
            }
            stream::Event::Failed { error } => app.failed = Some(error),
            stream::Event::Closed => app.failed = Some("stream closed".into()),
            _ => {}
        }
    }
}

fn main() -> Result<()> {
    clip_core::init()?;
    let filter = std::env::args().nth(1);

    let conn = Connection::connect_to_env()?;
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    conn.display().get_registry(&qh, ());
    let mut app = App::default();
    // Globals, then windows, then each window's properties.
    for _ in 0..3 {
        queue.roundtrip(&mut app)?;
    }

    println!("window management: {}", app.wm.is_some());
    println!("screencast:        {}", app.sc.is_some());
    let sc = app
        .sc
        .clone()
        .context("KWin didn't offer zkde_screencast_unstable_v1 (missing .desktop grant?)")?;

    let Some(filter) = filter else {
        for (uuid, w) in &app.windows {
            println!(
                "{uuid}  pid={:<7} fullscreen={:<5} app_id={:<30} {}",
                w.pid,
                w.state & 0x8 != 0,
                w.app_id,
                w.title
            );
        }
        return Ok(());
    };

    let (uuid, w) = app
        .windows
        .iter()
        .find(|(_, w)| w.app_id.contains(&filter))
        .with_context(|| format!("no window with app_id containing {filter:?}"))?;
    println!("streaming {uuid} ({} — {})", w.app_id, w.title);
    // Pointer mode 2 = embedded.
    sc.stream_window(uuid.clone(), 2, &qh, ());
    while app.serial.is_none() && app.failed.is_none() {
        queue.blocking_dispatch(&mut app)?;
    }
    if let Some(e) = app.failed {
        bail!("stream failed: {e}");
    }
    let serial = app.serial.unwrap();
    println!("pipewire object serial {serial}");

    // CLIPSH_LAUNCH runs a custom pipeline ("{serial}" is substituted, must
    // contain an element named "sink"); CLIPSH_SHOT=<file.png> saves one
    // frame; otherwise frames are counted.
    let custom = std::env::var("CLIPSH_LAUNCH").ok();
    let launch = match std::env::var("CLIPSH_SHOT") {
        _ if custom.is_some() => custom.unwrap().replace("{serial}", &serial.to_string()),
        Ok(out) => format!(
            "pipewiresrc target-object={serial} num-buffers=1 ! videoconvert ! pngenc ! identity name=sink ! filesink location={out}"
        ),
        Err(_) => format!(
            "pipewiresrc target-object={serial} num-buffers=120 ! videoconvert ! fakesink name=sink"
        ),
    };
    let pipeline = gst::parse::launch(&launch)?
        .downcast::<gst::Pipeline>()
        .unwrap();
    let caps = std::sync::Arc::new(std::sync::Mutex::new(None));
    {
        let caps = caps.clone();
        let pad = pipeline
            .by_name("sink")
            .unwrap()
            .static_pad("sink")
            .unwrap();
        pad.add_probe(gst::PadProbeType::EVENT_DOWNSTREAM, move |_, info| {
            if let Some(gst::PadProbeData::Event(e)) = &info.data
                && let gst::EventView::Caps(c) = e.view()
            {
                *caps.lock().unwrap() = Some(c.caps().to_string());
            }
            gst::PadProbeReturn::Ok
        });
    }
    pipeline.set_state(gst::State::Playing)?;
    let bus = pipeline.bus().unwrap();
    let start = std::time::Instant::now();
    // Keep servicing Wayland while frames flow.
    let result = loop {
        conn.flush()?;
        queue.dispatch_pending(&mut app)?;
        if let Some(msg) = bus.timed_pop_filtered(
            gst::ClockTime::from_mseconds(50),
            &[gst::MessageType::Eos, gst::MessageType::Error],
        ) {
            break match msg.view() {
                gst::MessageView::Eos(_) => Ok(()),
                gst::MessageView::Error(e) => {
                    Err(anyhow::anyhow!("{}: {:?}", e.error(), e.debug()))
                }
                _ => unreachable!(),
            };
        }
        if start.elapsed().as_secs() > 30 {
            break Err(anyhow::anyhow!("timed out waiting for frames"));
        }
    };
    if let Some(rate) = pipeline.by_name("rate") {
        println!(
            "videorate in={} out={} dup={} drop={}",
            rate.property::<u64>("in"),
            rate.property::<u64>("out"),
            rate.property::<u64>("duplicate"),
            rate.property::<u64>("drop")
        );
    }
    pipeline.set_state(gst::State::Null)?;
    result?;
    println!(
        "got 120 frames in {:.2}s, caps {}",
        start.elapsed().as_secs_f64(),
        caps.lock().unwrap().as_deref().unwrap_or("?")
    );
    Ok(())
}
