//! Automatic capture on Windows: poll for game windows; when one appears,
//! record that window (only it, like Medal) through Windows Graphics
//! Capture; follow it if the game replaces it; stop when it closes. Same
//! public surface as the Linux `auto` module.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use tokio::sync::mpsc;

pub use crate::backend::Backend;
use crate::{
    encoder::{Codec, Encoder},
    game::Game,
    recorder::{CaptureSource, Recorder, RecorderConfig, RestartPolicy, Status},
    save::{clip_path, screenshot_path},
    win::{Target, game::RunningGame},
};

/// How often windows are walked looking for games (and the recording's
/// health checked).
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
    /// The window being recorded.
    hwnd: isize,
    /// When this recording started.
    started: Instant,
    /// The game's display, if it is running in HDR.
    hdr: Option<crate::HdrDisplay>,
    /// Captured with HDR tone mapping (HDR compatibility).
    tonemapped: bool,
    recorder: Recorder,
}

pub struct AutoRecorder {
    cfg: RecorderConfig,
    probed: HashMap<Codec, Encoder>,
    running: Vec<RunningGame>,
    active: Option<Active>,
    /// When a failed recording may be restarted.
    restarts: RestartPolicy,
    /// The game whose recording is being restarted (after a failure or a
    /// new window): starting it again continues its play session.
    resume: Option<Game>,
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
                restarts: RestartPolicy::default(),
                resume: None,
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
                self.restarts.reset();
                return self.stop().await.map(|g| g.map(Change::Stopped));
            };
            if now.hwnd == a.hwnd {
                let Some(failure) = a.recorder.failure() else {
                    return Ok(None);
                };
                // The capture died. Left alone, every clip would replay the
                // last moments before the failure: restart it, backing off
                // when it keeps failing (each attempt opens an encoder
                // session, and drivers allow only a few).
                let healthy_for = a.started.elapsed();
                let _ = self.stop().await;
                let delay = self.note_failure(healthy_for, now.game.clone());
                tracing::warn!(
                    game = %now.game.name,
                    consecutive = self.restarts.failures(),
                    retry_in = ?delay,
                    "capture failed, restarting it: {failure}"
                );
                return Ok(None);
            }
            // Same game, new window (a splash screen gave way to the game,
            // or it recreated its window switching to fullscreen). The play
            // session carries on; the replay buffer restarts.
            tracing::info!(game = %now.game.name, "game window changed; following it");
            let _ = self.stop().await;
            self.resume = Some(now.game.clone());
        }

        // Not recording. A game whose recording is being restarted that
        // has since closed ends its session here.
        if let Some(g) = &self.resume
            && !self.running.iter().any(|r| &r.game == g)
        {
            self.restarts.reset();
            return Ok(self.resume.take().map(Change::Stopped));
        }
        if !self.restarts.due() {
            return Ok(None);
        }
        // The game being restarted first, else whatever is in front.
        let next = self
            .resume
            .as_ref()
            .and_then(|g| self.running.iter().find(|r| &r.game == g))
            .or(self.running.first())
            .cloned();
        let Some(next) = next else {
            return Ok(None);
        };
        match self.start(next.clone()).await {
            // A restart continues the same play session: no new "Started"
            // (which would also pop "Ready to record" over the game again).
            Ok(_) if self.resume.as_ref() == Some(&next.game) => {
                self.resume = None;
                Ok(None)
            }
            Ok(change) => {
                self.resume = None;
                Ok(Some(change))
            }
            Err(e) => {
                self.note_failure(Duration::ZERO, next.game);
                Err(e)
            }
        }
    }

    /// Count a failed recording or start and schedule the next attempt.
    /// `healthy_for`: how long the recording ran before failing; one that
    /// ran a good while resets the count.
    fn note_failure(&mut self, healthy_for: Duration, game: Game) -> Duration {
        self.resume = Some(game);
        self.restarts.failed(healthy_for)
    }

    async fn start(&mut self, running: RunningGame) -> Result<Change> {
        let mut cfg = self.cfg.clone();
        cfg.encoder = Some(self.resolve_encoder().await?);
        let hdr = crate::win::display::hdr_display(running.monitor);
        let window = Target {
            window: running.hwnd,
            cursor: cfg.cursor,
            hdr_monitor: None,
        };
        let mut tonemapped = false;
        let recorder = if let (true, Some(h)) = (cfg.hdr_compat, hdr) {
            tracing::info!(
                sdr_white_nits = h.sdr_white_nits,
                peak_nits = h.peak_nits,
                "the game's display is in HDR; capturing with tone mapping (HDR compatibility)"
            );
            let region = Target {
                hdr_monitor: Some(running.monitor),
                ..window
            };
            match Recorder::start(&cfg, CaptureSource::Screen(region)).await {
                Ok(r) => {
                    tonemapped = true;
                    r
                }
                Err(e) => {
                    tracing::warn!("HDR capture failed, using window capture: {e:#}");
                    Self::start_window(&cfg, window, &running.game).await?
                }
            }
        } else {
            if let Some(h) = hdr {
                tracing::info!(
                    sdr_white_nits = h.sdr_white_nits,
                    peak_nits = h.peak_nits,
                    "the game's display is in HDR; clips are SDR, so HDR highlights clip \
                     (turn on HDR compatibility to tone-map them)"
                );
            }
            Self::start_window(&cfg, window, &running.game).await?
        };
        tracing::info!(
            game = %running.game.name,
            hwnd = running.hwnd,
            tonemapped,
            "recording"
        );
        self.active = Some(Active {
            game: running.game.clone(),
            hwnd: running.hwnd,
            started: Instant::now(),
            hdr,
            tonemapped,
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

    /// Replace the recording settings. The replay length applies now, the
    /// rest from the next game.
    pub fn set_config(&mut self, cfg: RecorderConfig) {
        // The replay length applies to the running recording right away;
        // everything else (codec, bitrate, devices) needs a new pipeline.
        if let Some(a) = &mut self.active {
            a.recorder.set_replay_seconds(cfg.replay_seconds);
        }
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

    /// Whether the recording tone-maps HDR (HDR compatibility is on, the
    /// display is in HDR, and that capture started).
    pub fn hdr_tonemapped(&self) -> bool {
        self.active.as_ref().is_some_and(|a| a.tonemapped)
    }

    /// Window capture of `game`'s window. No fallback to capturing the
    /// monitor: that would record the whole screen, which window capture
    /// exists to avoid.
    async fn start_window(cfg: &RecorderConfig, target: Target, game: &Game) -> Result<Recorder> {
        Recorder::start(cfg, CaptureSource::Screen(target))
            .await
            .with_context(|| format!("can't capture the {} window", game.name))
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
