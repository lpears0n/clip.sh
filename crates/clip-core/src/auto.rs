//! Medal-style automatic capture: when a game starts, start buffering it;
//! when it ends, stop. Saving names the clip after the game.
//!
//! Two capture backends:
//! - **KWin** (KDE Plasma): games are detected per window and that window is
//!   streamed directly, with no picker.
//! - **Portal** (every other Wayland desktop): games are detected by scanning
//!   processes, and the monitor chosen once through the screen-cast portal is
//!   recorded. The portal's restore token makes later sessions silent.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::{
    encoder::{Codec, Encoder},
    game::{self, Game},
    kwin::{Kwin, WindowEvent, WindowInfo},
    portal::ScreenSource,
    recorder::{CaptureSource, Recorder, RecorderConfig, Status},
    save::clip_path,
};

/// How often the portal backend scans for running games.
const SCAN_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    Kwin,
    Portal,
}

/// Input for [`AutoRecorder::handle`].
#[derive(Debug, Clone)]
pub enum Event {
    /// KWin backend: a window appeared, changed or closed.
    Window(WindowEvent),
    /// Portal backend: the set of running games changed.
    Games(Vec<Game>),
}

/// What [`AutoRecorder::handle`] did in response to an event.
#[derive(Debug, Clone)]
pub enum Change {
    Started(Game),
    Stopped(Game),
}

struct Active {
    /// KWin: the recorded window. Portal: none (the monitor is recorded).
    window: Option<String>,
    game: Game,
    recorder: Recorder,
}

enum State {
    Kwin {
        kwin: Kwin,
        windows: HashMap<String, WindowInfo>,
        /// Detection result per window; `None` once checked and not a game.
        games: HashMap<String, Option<Game>>,
    },
    Portal {
        /// Where the portal's restore token is kept between runs.
        token_path: PathBuf,
        /// Games currently running, from the last scan.
        running: Vec<Game>,
    },
}

pub struct AutoRecorder {
    state: State,
    cfg: RecorderConfig,
    /// Best working encoder per codec, probed on first use.
    probed: HashMap<Codec, Encoder>,
    active: Option<Active>,
}

impl AutoRecorder {
    /// Pick a backend (`None` = KWin when available, else the portal) and
    /// start watching for games. Feed every event from the returned receiver
    /// to [`AutoRecorder::handle`].
    pub async fn new(
        cfg: RecorderConfig,
        prefer: Option<Backend>,
        token_path: PathBuf,
    ) -> Result<(Self, mpsc::UnboundedReceiver<Event>)> {
        let (tx, rx) = mpsc::unbounded_channel();
        let state = match prefer {
            Some(Backend::Portal) => portal_state(tx, token_path),
            Some(Backend::Kwin) => kwin_state(tx)?,
            None => match kwin_state(tx.clone()) {
                Ok(s) => s,
                Err(e) => {
                    tracing::info!(
                        "KWin capture unavailable ({e:#}); using the screen-cast portal"
                    );
                    portal_state(tx, token_path)
                }
            },
        };
        let me = Self {
            state,
            cfg,
            probed: HashMap::new(),
            active: None,
        };
        tracing::info!(backend = ?me.backend(), "watching for games");
        Ok((me, rx))
    }

    pub fn backend(&self) -> Backend {
        match self.state {
            State::Kwin { .. } => Backend::Kwin,
            State::Portal { .. } => Backend::Portal,
        }
    }

    /// Portal backend without a chosen screen: recording can't start until
    /// [`AutoRecorder::choose_screen`] runs (it would otherwise pop the picker
    /// up mid-game).
    pub fn needs_screen(&self) -> bool {
        match &self.state {
            State::Portal { token_path, .. } => read_token(token_path).is_none(),
            State::Kwin { .. } => false,
        }
    }

    /// Show the portal's screen picker now and remember the choice. If a game
    /// is already running, recording starts right away.
    pub async fn choose_screen(&mut self) -> Result<Option<Change>> {
        let State::Portal {
            token_path,
            running,
        } = &self.state
        else {
            bail!("only needed for portal capture");
        };
        let (token_path, running) = (token_path.clone(), running.clone());
        // A fresh session (no token) shows the picker.
        let source = ScreenSource::open_monitor(None).await?;
        save_token(&token_path, source.restore_token())?;
        source.close().await?;
        // A game may already be running; start recording it now.
        if self.active.is_none()
            && let Some(g) = running.first().cloned()
        {
            return self.start_portal(g).await.map(Some);
        }
        Ok(None)
    }

    pub async fn handle(&mut self, event: Event) -> Result<Option<Change>> {
        match event {
            Event::Window(ev) => self.handle_window(ev).await,
            Event::Games(games) => self.handle_games(games).await,
        }
    }

    async fn handle_games(&mut self, games: Vec<Game>) -> Result<Option<Change>> {
        let State::Portal {
            running,
            token_path,
        } = &mut self.state
        else {
            return Ok(None);
        };
        *running = games.clone();
        let have_token = read_token(token_path).is_some();
        if let Some(a) = &self.active
            && !games.contains(&a.game)
        {
            let stopped = self.stop().await?;
            if let Some(next) = games.first().cloned()
                && have_token
            {
                return self.start_portal(next).await.map(Some);
            }
            return Ok(stopped.map(Change::Stopped));
        }
        if self.active.is_none()
            && let Some(g) = games.first().cloned()
        {
            if !have_token {
                tracing::info!(game = %g.name, "game running, but no screen chosen yet");
                return Ok(None);
            }
            return self.start_portal(g).await.map(Some);
        }
        Ok(None)
    }

    async fn start_portal(&mut self, game: Game) -> Result<Change> {
        let State::Portal { token_path, .. } = &self.state else {
            bail!("not using the portal backend");
        };
        let token_path = token_path.clone();
        let token = read_token(&token_path);
        let source = ScreenSource::open_monitor(token.as_deref()).await?;
        // Portals issue a new token per session; keep the latest.
        save_token(&token_path, source.restore_token())?;
        let mut cfg = self.cfg.clone();
        cfg.encoder = Some(self.resolve_encoder().await?);
        let recorder = Recorder::start(&cfg, CaptureSource::Portal(source)).await?;
        tracing::info!(game = %game.name, "recording (screen)");
        self.active = Some(Active {
            window: None,
            game: game.clone(),
            recorder,
        });
        Ok(Change::Started(game))
    }

    async fn handle_window(&mut self, event: WindowEvent) -> Result<Option<Change>> {
        let State::Kwin { windows, games, .. } = &mut self.state else {
            return Ok(None);
        };
        match event {
            WindowEvent::Added(w) | WindowEvent::Changed(w) => {
                let uuid = w.uuid.clone();
                windows.insert(uuid.clone(), w);
                if self.active.is_none() && self.game_for(&uuid).is_some() {
                    return self.start_window(&uuid).await.map(Some);
                }
                Ok(None)
            }
            WindowEvent::Removed(uuid) => {
                windows.remove(&uuid);
                games.remove(&uuid);
                if self
                    .active
                    .as_ref()
                    .is_none_or(|a| a.window.as_deref() != Some(&uuid))
                {
                    return Ok(None);
                }
                let stopped = self.stop().await?;
                // Launchers often close a splash window as the game opens.
                let State::Kwin { windows, games, .. } = &self.state else {
                    unreachable!()
                };
                let next = windows
                    .keys()
                    .find(|u| games.get(*u).is_some_and(Option::is_some))
                    .cloned();
                if let Some(next) = next {
                    return self.start_window(&next).await.map(Some);
                }
                Ok(stopped.map(Change::Stopped))
            }
        }
    }

    /// Detect (once per window, after its pid is known) whether it's a game.
    fn game_for(&mut self, uuid: &str) -> Option<&Game> {
        let State::Kwin { windows, games, .. } = &mut self.state else {
            return None;
        };
        let w = windows.get(uuid)?;
        if w.pid == 0 {
            return None;
        }
        if !games.contains_key(uuid) {
            let g = game::detect(w.pid, &w.title);
            if let Some(g) = &g {
                tracing::info!(app_id = %w.app_id, pid = w.pid, game = %g.name, "game window detected");
            }
            games.insert(uuid.to_owned(), g);
        }
        games.get(uuid)?.as_ref()
    }

    async fn start_window(&mut self, uuid: &str) -> Result<Change> {
        let mut cfg = self.cfg.clone();
        cfg.encoder = Some(self.resolve_encoder().await?);
        let State::Kwin { kwin, games, .. } = &self.state else {
            bail!("not using the KWin backend");
        };
        let game = games[uuid].clone().context("not a game window")?;
        let stream = kwin.stream_window(uuid, cfg.cursor).await?;
        let recorder = Recorder::start(&cfg, CaptureSource::Kwin(stream)).await?;
        tracing::info!(game = %game.name, "recording (window)");
        self.active = Some(Active {
            window: Some(uuid.to_owned()),
            game: game.clone(),
            recorder,
        });
        Ok(Change::Started(game))
    }

    async fn stop(&mut self) -> Result<Option<Game>> {
        let Some(a) = self.active.take() else {
            return Ok(None);
        };
        a.recorder.stop().await?;
        tracing::info!(game = %a.game.name, "stopped recording");
        Ok(Some(a.game))
    }

    /// Replace the recording settings. Takes effect from the next game.
    pub fn set_config(&mut self, cfg: RecorderConfig) {
        self.cfg = cfg;
    }

    /// The configured encoder if it handles the codec, else the best one
    /// that does (probed once per codec).
    async fn resolve_encoder(&mut self) -> Result<Encoder> {
        let codec = self.cfg.video.codec;
        if let Some(e) = self.cfg.encoder
            && e.element(codec).is_some()
        {
            return Ok(e);
        }
        if let Some(e) = self.probed.get(&codec) {
            return Ok(*e);
        }
        let e = tokio::task::spawn_blocking(move || Encoder::probe(codec))
            .await?
            .with_context(|| format!("no working {codec:?} encoder found"))?;
        self.probed.insert(codec, e);
        Ok(e)
    }

    pub fn codec(&self) -> Codec {
        self.cfg.video.codec
    }

    /// Encoder used for the current (or most recent) recording.
    pub fn encoder(&self) -> Option<Encoder> {
        self.cfg
            .encoder
            .filter(|e| e.element(self.cfg.video.codec).is_some())
            .or_else(|| self.probed.get(&self.cfg.video.codec).copied())
    }

    pub fn current_game(&self) -> Option<&Game> {
        self.active.as_ref().map(|a| &a.game)
    }

    pub fn status(&self) -> Option<Status> {
        self.active.as_ref().map(|a| a.recorder.status())
    }

    /// Save the last `seconds` (default: the whole buffer) under
    /// `<root>/<game name>/`.
    pub async fn save(&self, root: &Path, seconds: Option<u32>) -> Result<PathBuf> {
        let a = self.active.as_ref().context("no game is being recorded")?;
        a.recorder
            .save(seconds, &clip_path(root, &a.game.name))
            .await
    }

    pub async fn shutdown(mut self) -> Result<()> {
        self.stop().await.map(|_| ())
    }
}

fn kwin_state(tx: mpsc::UnboundedSender<Event>) -> Result<State> {
    let (kwin, mut events) = Kwin::connect()?;
    tokio::spawn(async move {
        while let Some(ev) = events.recv().await {
            if tx.send(Event::Window(ev)).is_err() {
                break;
            }
        }
    });
    Ok(State::Kwin {
        kwin,
        windows: HashMap::new(),
        games: HashMap::new(),
    })
}

/// Scan for running games and report whenever the set changes.
fn portal_state(tx: mpsc::UnboundedSender<Event>, token_path: PathBuf) -> State {
    tokio::spawn(async move {
        let mut last: Option<Vec<Game>> = None;
        loop {
            let games = tokio::task::spawn_blocking(game::running_games)
                .await
                .unwrap_or_default();
            if last.as_ref() != Some(&games) {
                if tx.send(Event::Games(games.clone())).is_err() {
                    break;
                }
                last = Some(games);
            }
            tokio::time::sleep(SCAN_INTERVAL).await;
        }
    });
    State::Portal {
        token_path,
        running: vec![],
    }
}

fn read_token(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

fn save_token(path: &Path, token: Option<&str>) -> Result<()> {
    let Some(token) = token else {
        // The portal didn't offer persistence; nothing to remember.
        return Ok(());
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, token).with_context(|| format!("writing {}", path.display()))
}
