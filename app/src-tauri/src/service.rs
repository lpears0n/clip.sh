//! Background task that owns the recorder. The UI talks to it through
//! [`Service`]; it reports changes as Tauri events.

use std::path::PathBuf;

use clip_core::{
    RecorderConfig,
    auto::{AutoRecorder, Backend, Change},
};
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::sync::{mpsc, oneshot};

use crate::{games, library};

/// Event names emitted to the frontend.
pub const STATUS_CHANGED: &str = "status-changed";
pub const CLIP_SAVED: &str = "clip-saved";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// Automatic capture isn't possible (e.g. not KDE); see `error`.
    Unavailable,
    /// Watching for a game window.
    Waiting,
    Recording,
}

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub state: State,
    pub game: Option<String>,
    pub steam_app_id: Option<u32>,
    /// When the current play session began (ms since epoch).
    pub session_start_ms: Option<u64>,
    pub buffered_seconds: f64,
    pub replay_seconds: u32,
    pub encoder: Option<String>,
    /// "zerocopy" (DMA-BUF into GPU memory) or "copy", while recording.
    pub capture_path: Option<clip_core::CapturePath>,
    pub error: Option<String>,
    /// How games are captured: "kwin" (window, KDE) or "portal" (screen).
    pub backend: Option<Backend>,
    /// Portal capture needs a screen chosen once before it can record.
    pub needs_screen: bool,
}

enum Cmd {
    Status(oneshot::Sender<Status>),
    Save(PathBuf, oneshot::Sender<Result<PathBuf, String>>),
    Configure(RecorderConfig),
    ChooseScreen(oneshot::Sender<Result<(), String>>),
}

#[derive(Clone)]
pub struct Service {
    tx: mpsc::UnboundedSender<Cmd>,
}

impl Service {
    /// `prefer`: capture backend to use (`None` = KWin when available, else
    /// the portal). `token_path`: where the portal's restore token is kept.
    pub fn spawn(
        app: AppHandle,
        cfg: RecorderConfig,
        prefer: Option<Backend>,
        token_path: PathBuf,
    ) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        tauri::async_runtime::spawn(run(app, cfg, prefer, token_path, rx));
        Self { tx }
    }

    /// Show the desktop's screen picker once (portal capture).
    pub async fn choose_screen(&self) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        self.send(Cmd::ChooseScreen(tx))?;
        rx.await.map_err(|_| "recorder stopped".to_string())?
    }

    pub async fn status(&self) -> Result<Status, String> {
        let (tx, rx) = oneshot::channel();
        self.send(Cmd::Status(tx))?;
        rx.await.map_err(|_| "recorder stopped".to_string())
    }

    /// Save the replay buffer into `clips_dir/<game>/`.
    pub async fn save(&self, clips_dir: PathBuf) -> Result<PathBuf, String> {
        let (tx, rx) = oneshot::channel();
        self.send(Cmd::Save(clips_dir, tx))?;
        rx.await.map_err(|_| "recorder stopped".to_string())?
    }

    pub fn configure(&self, cfg: RecorderConfig) -> Result<(), String> {
        self.send(Cmd::Configure(cfg))
    }

    fn send(&self, cmd: Cmd) -> Result<(), String> {
        self.tx
            .send(cmd)
            .map_err(|_| "recorder stopped".to_string())
    }
}

async fn run(
    app: AppHandle,
    cfg: RecorderConfig,
    prefer: Option<Backend>,
    token_path: PathBuf,
    mut cmds: mpsc::UnboundedReceiver<Cmd>,
) {
    let replay_seconds = cfg.replay_seconds;
    let (mut auto, mut events) = match AutoRecorder::new(cfg, prefer, token_path).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("automatic capture unavailable: {e:#}");
            let status = Status {
                state: State::Unavailable,
                game: None,
                steam_app_id: None,
                session_start_ms: None,
                buffered_seconds: 0.0,
                replay_seconds,
                encoder: None,
                capture_path: None,
                error: Some(format!("{e:#}")),
                backend: None,
                needs_screen: false,
            };
            while let Some(cmd) = cmds.recv().await {
                match cmd {
                    Cmd::Status(r) => drop(r.send(status.clone())),
                    Cmd::Save(_, r) => drop(r.send(Err("automatic capture is unavailable".into()))),
                    Cmd::Configure(_) => {}
                    Cmd::ChooseScreen(r) => {
                        drop(r.send(Err("automatic capture is unavailable".into())))
                    }
                }
            }
            return;
        }
    };
    let mut replay_seconds = replay_seconds;
    // The game being played and when its session started.
    let mut session: Option<(clip_core::game::Game, u64)> = None;

    loop {
        tokio::select! {
            ev = events.recv() => {
                let Some(ev) = ev else { break };
                match auto.handle(ev).await {
                    Ok(Some(change)) => {
                        apply(change, &mut session);
                        let _ = app.emit(STATUS_CHANGED, status(&auto, replay_seconds, session.as_ref()));
                    }
                    Ok(None) => {}
                    Err(e) => tracing::error!("{e:#}"),
                }
            }
            cmd = cmds.recv() => {
                let Some(cmd) = cmd else { break };
                match cmd {
                    Cmd::Status(r) => drop(r.send(status(&auto, replay_seconds, session.as_ref()))),
                    Cmd::Save(dir, r) => {
                        let res = auto.save(&dir, None).await.map_err(|e| format!("{e:#}"));
                        if let Ok(p) = &res {
                            if let Some(g) = auto.current_game() {
                                let meta = library::ClipMeta {
                                    game: g.name.clone(),
                                    steam_app_id: g.steam_app_id,
                                    ..Default::default()
                                };
                                if let Err(e) = library::write_meta(p, &meta) {
                                    tracing::warn!("clip metadata: {e:#}");
                                }
                            }
                            let _ = app.emit(CLIP_SAVED, p);
                        }
                        drop(r.send(res));
                    }
                    Cmd::ChooseScreen(r) => {
                        let res = match auto.choose_screen().await {
                            Ok(change) => {
                                if let Some(c) = change {
                                    apply(c, &mut session);
                                }
                                Ok(())
                            }
                            Err(e) => Err(format!("{e:#}")),
                        };
                        drop(r.send(res));
                        let _ = app.emit(STATUS_CHANGED, status(&auto, replay_seconds, session.as_ref()));
                    }
                    Cmd::Configure(cfg) => {
                        replay_seconds = cfg.replay_seconds;
                        auto.set_config(cfg);
                    }
                }
            }
        }
    }
    end_session(session.take());
    let _ = auto.shutdown().await;
}

/// Track play sessions as recording starts and stops.
fn apply(change: Change, session: &mut Option<(clip_core::game::Game, u64)>) {
    match change {
        Change::Started(g) => {
            tracing::info!("recording {}", g.name);
            end_session(session.take());
            *session = Some((g, games::now_ms()));
        }
        Change::Stopped(g) => {
            tracing::info!("stopped recording {}", g.name);
            end_session(session.take());
        }
    }
}

/// Log a finished play session.
fn end_session(session: Option<(clip_core::game::Game, u64)>) {
    let Some((g, start_ms)) = session else { return };
    let s = games::Session {
        game: g.name,
        steam_app_id: g.steam_app_id,
        start_ms,
        end_ms: games::now_ms(),
    };
    if let Err(e) = games::record(&s) {
        tracing::warn!("session log: {e:#}");
    }
}

fn status(
    auto: &AutoRecorder,
    replay_seconds: u32,
    session: Option<&(clip_core::game::Game, u64)>,
) -> Status {
    let rec = auto.status();
    Status {
        state: if rec.is_some() {
            State::Recording
        } else {
            State::Waiting
        },
        game: auto.current_game().map(|g| g.name.clone()),
        steam_app_id: auto.current_game().and_then(|g| g.steam_app_id),
        session_start_ms: session.map(|(_, t)| *t),
        buffered_seconds: rec.as_ref().map_or(0.0, |s| s.buffered_seconds),
        replay_seconds: rec.as_ref().map_or(replay_seconds, |s| s.replay_seconds),
        encoder: auto
            .encoder()
            .and_then(|e| e.element(auto.codec()))
            .map(str::to_owned),
        capture_path: rec.as_ref().map(|s| s.path),
        error: rec.and_then(|s| s.error),
        backend: Some(auto.backend()),
        needs_screen: auto.needs_screen(),
    }
}
