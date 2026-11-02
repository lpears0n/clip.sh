//! Automatic capture on Windows: poll for game windows; when one appears,
//! record the monitor it's on through Windows Graphics Capture; stop when
//! it closes. Same public surface as the Linux `auto` module.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use tokio::sync::mpsc;

pub use crate::backend::Backend;
use crate::{
    encoder::{Codec, Encoder},
    game::Game,
    recorder::{CaptureSource, Recorder, RecorderConfig, Status},
    save::{clip_path, screenshot_path},
    win::{Target, game::RunningGame},
};

/// How often windows are walked looking for games.
const SCAN_INTERVAL: Duration = Duration::from_secs(2);

/// Input for [`AutoRecorder::handle`].
#[derive(Debug, Clone)]
pub enum Event {
    /// The set of game windows changed.
    Games(Vec<RunningGame>),
}

/// What [`AutoRecorder::handle`] did in response to an event.
#[derive(Debug, Clone)]
pub enum Change {
    Started(Game),
    Stopped(Game),
}

struct Active {
    game: Game,
    recorder: Recorder,
}

pub struct AutoRecorder {
    cfg: RecorderConfig,
    probed: HashMap<Codec, Encoder>,
    running: Vec<RunningGame>,
    active: Option<Active>,
}

impl AutoRecorder {
    /// Start watching for games. `prefer` and `token_path` exist for parity
    /// with Linux; Windows has one backend and needs no token.
    pub async fn new(
        cfg: RecorderConfig,
        _prefer: Option<Backend>,
        _token_path: PathBuf,
    ) -> Result<(Self, mpsc::UnboundedReceiver<Event>)> {
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut last: Option<Vec<RunningGame>> = None;
            loop {
                let games = tokio::task::spawn_blocking(crate::win::game::running_games)
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
        tracing::info!(backend = ?Backend::Windows, "watching for games");
        Ok((
            Self {
                cfg,
                probed: HashMap::new(),
                running: vec![],
                active: None,
            },
            rx,
        ))
    }

    pub fn backend(&self) -> Backend {
        Backend::Windows
    }

    /// Windows Graphics Capture needs no prior permission.
    pub fn needs_screen(&self) -> bool {
        false
    }

    pub async fn choose_screen(&mut self) -> Result<Option<Change>> {
        bail!("the screen is chosen automatically on Windows")
    }

    pub async fn handle(&mut self, event: Event) -> Result<Option<Change>> {
        let Event::Games(games) = event;
        self.running = games;
        if let Some(a) = &self.active {
            if self.running.iter().any(|g| g.game == a.game) {
                return Ok(None);
            }
            return self.stop().await.map(|g| g.map(Change::Stopped));
        }
        let Some(next) = self.running.first().cloned() else {
            return Ok(None);
        };
        self.start(next).await.map(Some)
    }

    async fn start(&mut self, running: RunningGame) -> Result<Change> {
        let mut cfg = self.cfg.clone();
        cfg.encoder = Some(self.resolve_encoder().await?);
        let target = Target {
            monitor: running.monitor,
            cursor: cfg.cursor,
        };
        let recorder = Recorder::start(&cfg, CaptureSource::Screen(target)).await?;
        tracing::info!(game = %running.game.name, "recording (monitor)");
        self.active = Some(Active {
            game: running.game.clone(),
            recorder,
        });
        Ok(Change::Started(running.game))
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

    pub async fn save(&self, root: &Path, seconds: Option<u32>) -> Result<PathBuf> {
        let a = self.active.as_ref().context("no game is being recorded")?;
        a.recorder
            .save(seconds, &clip_path(root, &a.game.name))
            .await
    }

    /// Save the newest frame as `<root>/<game name>/<timestamp>.png`.
    pub async fn screenshot(&self, root: &Path) -> Result<PathBuf> {
        let a = self.active.as_ref().context("no game is being recorded")?;
        a.recorder
            .screenshot(&screenshot_path(root, &a.game.name))
            .await
    }

    pub async fn shutdown(mut self) -> Result<()> {
        self.stop().await.map(|_| ())
    }
}
