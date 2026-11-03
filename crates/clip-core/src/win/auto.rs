//! Automatic capture on Windows: poll for game windows; when one appears,
//! record that window (only it, like Medal) through Windows Graphics
//! Capture; follow it if the game replaces it; stop when it closes. Same
//! public surface as the Linux `auto` module.

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

/// How often windows are walked looking for games (and the recording's
/// health checked).
const SCAN_INTERVAL: Duration = Duration::from_secs(2);

/// After a failed start, how long to wait before trying again.
const RETRY_AFTER: Duration = Duration::from_secs(10);

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
    /// The window being recorded.
    hwnd: isize,
    /// The game's display, if it is running in HDR.
    hdr: Option<crate::HdrDisplay>,
    recorder: Recorder,
}

pub struct AutoRecorder {
    cfg: RecorderConfig,
    probed: HashMap<Codec, Encoder>,
    running: Vec<RunningGame>,
    active: Option<Active>,
    /// No new start attempt before this, after one failed.
    retry_at: Option<std::time::Instant>,
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
            let mut scanner = crate::win::game::Scanner::default();
            loop {
                let games = match tokio::task::spawn_blocking(move || {
                    let games = scanner.scan();
                    (scanner, games)
                })
                .await
                {
                    Ok((s, games)) => {
                        scanner = s;
                        games
                    }
                    // A panicking scan loses its cache, not the watcher.
                    Err(_) => {
                        scanner = crate::win::game::Scanner::default();
                        Vec::new()
                    }
                };
                // Every scan, changed or not: each one also checks that the
                // recording is still alive.
                if tx.send(Event::Games(games)).is_err() {
                    break;
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
                retry_at: None,
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
            let Some(now) = self.running.iter().find(|g| g.game == a.game).cloned() else {
                return self.stop().await.map(|g| g.map(Change::Stopped));
            };
            if now.hwnd == a.hwnd {
                let Some(failure) = a.recorder.failure() else {
                    return Ok(None);
                };
                // The capture died (the window's capture was closed, the
                // GPU was reset, ...). Left alone, every clip would replay
                // the last moments before the failure.
                tracing::warn!(game = %now.game.name, "capture failed, restarting it: {failure}");
            } else {
                // Same game, new window (a splash screen gave way to the
                // game, or it recreated its window switching to fullscreen).
                tracing::info!(game = %now.game.name, "game window changed; following it");
            }
            // Either way the play session carries on; the replay buffer
            // restarts.
            let _ = self.stop().await;
            self.try_start(now).await?;
            return Ok(None);
        }
        if self.retry_at.is_some_and(|t| std::time::Instant::now() < t) {
            return Ok(None);
        }
        let Some(next) = self.running.first().cloned() else {
            return Ok(None);
        };
        self.try_start(next).await.map(Some)
    }

    /// [`Self::start`], backing off for a while if it fails, so a game that
    /// can't be captured isn't retried every scan.
    async fn try_start(&mut self, running: RunningGame) -> Result<Change> {
        match self.start(running).await {
            Ok(c) => {
                self.retry_at = None;
                Ok(c)
            }
            Err(e) => {
                self.retry_at = Some(std::time::Instant::now() + RETRY_AFTER);
                Err(e)
            }
        }
    }

    async fn start(&mut self, running: RunningGame) -> Result<Change> {
        let mut cfg = self.cfg.clone();
        cfg.encoder = Some(self.resolve_encoder().await?);
        let target = Target {
            window: running.hwnd,
            cursor: cfg.cursor,
        };
        // No fallback to capturing the monitor: that would record the
        // whole screen, which window capture exists to avoid.
        let recorder = Recorder::start(&cfg, CaptureSource::Screen(target))
            .await
            .with_context(|| format!("can't capture the {} window", running.game.name))?;
        tracing::info!(game = %running.game.name, hwnd = running.hwnd, "recording (window)");
        let hdr = crate::win::display::hdr_display(running.monitor);
        if let Some(h) = hdr {
            tracing::info!(
                sdr_white_nits = h.sdr_white_nits,
                peak_nits = h.peak_nits,
                "the game's display is in HDR; clips are SDR, so HDR highlights clip"
            );
        }
        self.active = Some(Active {
            game: running.game.clone(),
            hwnd: running.hwnd,
            hdr,
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

    /// The recorded game's display, if it is running in HDR.
    pub fn hdr(&self) -> Option<crate::HdrDisplay> {
        self.active.as_ref().and_then(|a| a.hdr)
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
