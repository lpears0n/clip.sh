//! KDE Plasma (KWin) integration: watch windows and screencast them without
//! the portal's picker.
//!
//! Both protocols are privileged. KWin may only offer them to executables
//! whose installed .desktop file lists them in `X-KDE-Wayland-Interfaces`
//! (see `packaging/`), so treat a missing global as "not on KDE / not granted"
//! and fall back to the portal.

pub mod protocols;
pub mod shortcut;

use std::{collections::HashMap, sync::mpsc, thread::JoinHandle, time::Duration};

use anyhow::{Context, Result, anyhow, bail};
use protocols::{
    screencast::{zkde_screencast_stream_unstable_v1 as stream, zkde_screencast_unstable_v1 as sc},
    window_management::{org_kde_plasma_window as win, org_kde_plasma_window_management as wm},
};
use rustix::event::{PollFd, PollFlags, poll};
use tokio::sync::{mpsc as tmpsc, oneshot};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, protocol::wl_registry};

const STATE_ACTIVE: u32 = 0x1;
const STATE_MINIMIZED: u32 = 0x2;
const STATE_FULLSCREEN: u32 = 0x8;
/// `zkde_screencast_unstable_v1` pointer modes.
const POINTER_HIDDEN: u32 = 1;
const POINTER_EMBEDDED: u32 = 2;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WindowInfo {
    /// KWin's stable window id.
    pub uuid: String,
    pub app_id: String,
    pub title: String,
    pub pid: u32,
    pub active: bool,
    pub minimized: bool,
    pub fullscreen: bool,
}

#[derive(Debug, Clone)]
pub enum WindowEvent {
    /// A window exists (sent for every window already open at connect too).
    Added(WindowInfo),
    Changed(WindowInfo),
    Removed(String),
}

/// A live window screencast. Read it with
/// `pipewiresrc target-object=<serial>`; dropping this ends the stream.
pub struct KwinStream {
    serial: u64,
    id: u32,
    cmds: mpsc::Sender<Cmd>,
}

impl KwinStream {
    pub fn pipewire_serial(&self) -> u64 {
        self.serial
    }
}

impl Drop for KwinStream {
    fn drop(&mut self) {
        let _ = self.cmds.send(Cmd::Close(self.id));
    }
}

/// Answer to a stream request: (PipeWire serial, stream id).
type StreamReply = oneshot::Sender<Result<(u64, u32)>>;

enum Cmd {
    StreamWindow(String, u32, StreamReply),
    Close(u32),
    Quit,
}

/// Handle to the KWin client thread.
pub struct Kwin {
    cmds: mpsc::Sender<Cmd>,
    thread: Option<JoinHandle<()>>,
}

impl Kwin {
    /// Connect to the compositor. Fails if KWin doesn't offer both protocols.
    /// Window events, starting with every existing window, arrive on the
    /// returned receiver.
    pub fn connect() -> Result<(Self, tmpsc::UnboundedReceiver<WindowEvent>)> {
        let conn = Connection::connect_to_env().context("no Wayland compositor")?;
        let mut queue = conn.new_event_queue();
        let qh = queue.handle();
        conn.display().get_registry(&qh, ());
        let (events_tx, events_rx) = tmpsc::unbounded_channel();
        let mut state = State {
            wm: None,
            sc: None,
            windows: HashMap::new(),
            streams: HashMap::new(),
            events: events_tx,
        };
        queue.roundtrip(&mut state)?;
        if state.wm.is_none() || state.sc.is_none() {
            bail!(
                "KWin screencast/window-management protocols not offered \
                 (not KDE, or the .desktop grant is missing)"
            );
        }

        let (cmds_tx, cmds_rx) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("clipsh-kwin".into())
            .spawn(move || {
                if let Err(e) = run(conn, queue, state, cmds_rx) {
                    tracing::error!("KWin connection ended: {e:#}");
                }
            })?;
        Ok((
            Self {
                cmds: cmds_tx,
                thread: Some(thread),
            },
            events_rx,
        ))
    }

    /// Start screencasting a window by its uuid, optionally drawing the
    /// cursor into the frames.
    pub async fn stream_window(&self, uuid: &str, cursor: bool) -> Result<KwinStream> {
        let (tx, rx) = oneshot::channel();
        let pointer = if cursor {
            POINTER_EMBEDDED
        } else {
            POINTER_HIDDEN
        };
        self.cmds
            .send(Cmd::StreamWindow(uuid.to_owned(), pointer, tx))
            .map_err(|_| anyhow!("KWin thread has exited"))?;
        let (serial, id) = tokio::time::timeout(Duration::from_secs(5), rx)
            .await
            .context("KWin didn't answer the stream request")?
            .context("KWin thread has exited")??;
        Ok(KwinStream {
            serial,
            id,
            cmds: self.cmds.clone(),
        })
    }
}

impl Drop for Kwin {
    fn drop(&mut self) {
        let _ = self.cmds.send(Cmd::Quit);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct State {
    wm: Option<wm::OrgKdePlasmaWindowManagement>,
    sc: Option<sc::ZkdeScreencastUnstableV1>,
    /// Windows whose initial state has arrived are reported; others pending.
    windows: HashMap<String, (WindowInfo, bool)>,
    /// Open streams by protocol id, with the reply for a pending request.
    streams: HashMap<u32, (stream::ZkdeScreencastStreamUnstableV1, Option<StreamReply>)>,
    events: tmpsc::UnboundedSender<WindowEvent>,
}

fn run(
    conn: Connection,
    mut queue: EventQueue<State>,
    mut state: State,
    cmds: mpsc::Receiver<Cmd>,
) -> Result<()> {
    let qh = queue.handle();
    loop {
        // Commands are polled; 50ms of latency is fine for stream setup.
        loop {
            match cmds.try_recv() {
                Ok(Cmd::StreamWindow(uuid, pointer, reply)) => {
                    let s = state
                        .sc
                        .as_ref()
                        .unwrap()
                        .stream_window(uuid, pointer, &qh, ());
                    state.streams.insert(s.id().protocol_id(), (s, Some(reply)));
                }
                Ok(Cmd::Close(id)) => {
                    if let Some((s, _)) = state.streams.remove(&id) {
                        s.close();
                    }
                }
                Ok(Cmd::Quit) | Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }

        queue.dispatch_pending(&mut state)?;
        conn.flush()?;
        if let Some(guard) = queue.prepare_read() {
            let timeout = rustix::event::Timespec {
                tv_sec: 0,
                tv_nsec: 50_000_000,
            };
            let ready = {
                let fd = guard.connection_fd();
                let mut fds = [PollFd::new(&fd, PollFlags::IN)];
                poll(&mut fds, Some(&timeout))? > 0
            };
            if ready {
                match guard.read() {
                    Ok(_) => {}
                    // Readable can still mean "nothing to read yet" (e.g.
                    // another thread took the data); that's not an error.
                    Err(wayland_client::backend::WaylandError::Io(e))
                        if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(e) => return Err(e.into()),
                }
            }
        }
        queue.dispatch_pending(&mut state)?;
        if state.events.is_closed() && state.streams.is_empty() {
            return Ok(());
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
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
                // v16: window_with_uuid (13), pid (8), initial_state (4).
                "org_kde_plasma_window_management" if version >= 13 => {
                    state.wm = Some(reg.bind(name, version.min(16), qh, ()));
                }
                // v6: PipeWire object serial instead of node id.
                "zkde_screencast_unstable_v1" if version >= 6 => {
                    state.sc = Some(reg.bind(name, 6, qh, ()));
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<wm::OrgKdePlasmaWindowManagement, ()> for State {
    fn event(
        state: &mut Self,
        wm: &wm::OrgKdePlasmaWindowManagement,
        event: wm::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wm::Event::WindowWithUuid { uuid, .. } = event {
            wm.get_window_by_uuid(uuid.clone(), qh, uuid.clone());
            let info = WindowInfo {
                uuid: uuid.clone(),
                ..Default::default()
            };
            state.windows.insert(uuid, (info, false));
        }
    }
}

impl Dispatch<win::OrgKdePlasmaWindow, String> for State {
    fn event(
        state: &mut Self,
        proxy: &win::OrgKdePlasmaWindow,
        event: win::Event,
        uuid: &String,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let win::Event::Unmapped = event {
            if let Some((_, reported)) = state.windows.remove(uuid)
                && reported
            {
                let _ = state.events.send(WindowEvent::Removed(uuid.clone()));
            }
            proxy.destroy();
            return;
        }
        let Some((w, reported)) = state.windows.get_mut(uuid) else {
            return;
        };
        let before = w.clone();
        match event {
            win::Event::TitleChanged { title } => w.title = title,
            win::Event::AppIdChanged { app_id } => w.app_id = app_id,
            win::Event::PidChanged { pid } => w.pid = pid,
            win::Event::StateChanged { flags } => {
                w.active = flags & STATE_ACTIVE != 0;
                w.minimized = flags & STATE_MINIMIZED != 0;
                w.fullscreen = flags & STATE_FULLSCREEN != 0;
            }
            win::Event::InitialState => {
                *reported = true;
                let _ = state.events.send(WindowEvent::Added(w.clone()));
                return;
            }
            _ => return,
        }
        if *reported && *w != before {
            let _ = state.events.send(WindowEvent::Changed(w.clone()));
        }
    }
}

impl Dispatch<sc::ZkdeScreencastUnstableV1, ()> for State {
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

impl Dispatch<stream::ZkdeScreencastStreamUnstableV1, ()> for State {
    fn event(
        state: &mut Self,
        proxy: &stream::ZkdeScreencastStreamUnstableV1,
        event: stream::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let id = proxy.id().protocol_id();
        let Some((_, reply)) = state.streams.get_mut(&id) else {
            return;
        };
        match event {
            stream::Event::Serial {
                object_serial_hi,
                object_serial_low,
            } => {
                let serial = ((object_serial_hi as u64) << 32) | object_serial_low as u64;
                if let Some(r) = reply.take() {
                    let _ = r.send(Ok((serial, id)));
                }
            }
            stream::Event::Failed { error } => {
                if let Some(r) = reply.take() {
                    let _ = r.send(Err(anyhow!("KWin refused the stream: {error}")));
                }
                state.streams.remove(&id);
            }
            stream::Event::Closed => {
                tracing::debug!(id, "KWin closed a stream");
                state.streams.remove(&id);
            }
            _ => {}
        }
    }
}
