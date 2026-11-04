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
    recorder::{CaptureSource, Recorder, RecorderConfig, RestartPolicy, SavedClip, Status},
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
    /// Through HDR window capture (experimental) rather than the screen.
    hdr_window: bool,
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
    /// HDR window capture failed while recording; the screen-area HDR
    /// capture is used instead until the app restarts.
    hdr_window_broken: bool,
    /// Encoders that passed the probe but kept failing while recording
    /// (NVENC refusing a session, say); skipped until the app restarts.
    broken_encoders: Vec<Encoder>,
    /// Failures in a row that the encoder itself was blamed for.
    encoder_failures: u32,
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
                hdr_window_broken: false,
                broken_encoders: Vec::new(),
                encoder_failures: 0,
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
        let Event::Games(mut games) = event;
        // Apps marked "not a game" don't exist as far as recording goes; a
        // game marked while it records stops on this scan.
        games.retain(|g| !crate::game::is_ignored(&self.cfg.ignored, &g.game.name, Some(&g.exe)));
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
                let encoder = a.recorder.status().encoder;
                let hdr_window = a.hdr_window;
                if healthy_for >= crate::recorder::HEALTHY_AFTER {
                    self.encoder_failures = 0;
                }
                self.blame_encoder(encoder, &failure);
                if hdr_window && !self.hdr_window_broken {
                    tracing::warn!(
                        "HDR window capture failed; using the screen-area HDR capture from now on"
                    );
                    self.hdr_window_broken = true;
                }
                let _ = self.stop().await;
                let delay = self.note_failure(healthy_for, now.game.clone(), failure.clone());
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
                self.note_failure(Duration::ZERO, next.game, format!("{e:#}"));
                Err(e)
            }
        }
    }

    /// Count a failed recording or start and schedule the next attempt.
    /// `healthy_for`: how long the recording ran before failing; one that
    /// ran a good while resets the count.
    fn note_failure(&mut self, healthy_for: Duration, game: Game, reason: String) -> Duration {
        self.resume = Some(game);
        self.restarts.set_reason(reason);
        self.restarts.failed(healthy_for)
    }

    /// Count failures the encoder itself is named in. Twice in a row and it
    /// is set aside for the next encoder that passes the probe: a driver
    /// that refuses sessions mid-game would otherwise retry forever with
    /// nothing recorded.
    fn blame_encoder(&mut self, encoder: Encoder, failure: &str) {
        let codec = self.cfg.video.codec;
        if !encoder.is_blamed_in(codec, failure) {
            self.encoder_failures = 0;
            return;
        }
        self.encoder_failures += 1;
        if self.encoder_failures < 2 {
            return;
        }
        tracing::warn!(
            ?encoder,
            "the encoder keeps failing while recording; switching to the next one that works \
             (another app may be using it, or the driver refuses more sessions)"
        );
        self.broken_encoders.push(encoder);
        self.probed.remove(&codec);
        self.encoder_failures = 0;
    }

    async fn start(&mut self, running: RunningGame) -> Result<Change> {
        let mut cfg = self.cfg.clone();
        cfg.encoder = Some(self.resolve_encoder().await?);
        let hdr = crate::win::display::hdr_display(running.monitor);
        let window = Target {
            window: running.hwnd,
            cursor: cfg.cursor,
            hdr_monitor: None,
            hdr_window: None,
        };
        let mut tonemapped = false;
        let mut hdr_window = false;
        let recorder = if let (true, Some(h)) = (cfg.hdr_compat, hdr) {
            tracing::info!(
                sdr_white_nits = h.sdr_white_nits,
                peak_nits = h.peak_nits,
                "the game's display is in HDR; capturing with tone mapping (HDR compatibility)"
            );
            // The window itself in HDR first (experimental), then the
            // window's area of the screen, then plain window capture.
            let mut started = None;
            if cfg.hdr_window && !self.hdr_window_broken {
                let target = Target {
                    hdr_window: Some(crate::win::HdrWindow {
                        display: h,
                        adapter_luid: crate::win::display::adapter_luid(running.monitor),
                    }),
                    ..window
                };
                match Recorder::start(&cfg, CaptureSource::Screen(target)).await {
                    Ok(r) => {
                        hdr_window = true;
                        started = Some(r);
                    }
                    Err(e) => {
                        tracing::warn!("HDR window capture failed, using the screen area: {e:#}");
                        self.hdr_window_broken = true;
                    }
                }
            }
            let region = Target {
                hdr_monitor: Some(running.monitor),
                ..window
            };
            match started {
                Some(r) => {
                    tonemapped = true;
                    r
                }
                None => match Recorder::start(&cfg, CaptureSource::Screen(region)).await {
                    Ok(r) => {
                        tonemapped = true;
                        r
                    }
                    Err(e) => {
                        tracing::warn!("HDR capture failed, using window capture: {e:#}");
                        Self::start_window(&cfg, window, &running.game).await?
                    }
                },
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
            hdr_window,
            "recording"
        );
        self.active = Some(Active {
            game: running.game.clone(),
            hwnd: running.hwnd,
            started: Instant::now(),
            hdr,
            tonemapped,
            hdr_window,
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
            && !self.broken_encoders.contains(&e)
        {
            return Ok(e);
        }
        if let Some(e) = self.probed.get(&codec) {
            return Ok(*e);
        }
        let skip = self.broken_encoders.clone();
        let e = tokio::task::spawn_blocking(move || Encoder::probe_excluding(codec, &skip))
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
            .filter(|e| {
                e.element(self.cfg.video.codec).is_some() && !self.broken_encoders.contains(e)
            })
            .or_else(|| self.probed.get(&self.cfg.video.codec).copied())
    }

    /// What to put on the ignore list to stop recording the current game
    /// for good ("not a game").
    pub fn ignore_entry(&self) -> Option<crate::game::IgnoredGame> {
        let a = self.active.as_ref()?;
        let exe = self
            .running
            .iter()
            .find(|g| g.game == a.game)
            .map(|g| g.exe.clone());
        Some(crate::game::IgnoredGame {
            name: a.game.name.clone(),
            exe,
        })
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

    /// The game whose capture failed and is waiting to be restarted.
    pub fn retrying(&self) -> Option<crate::recorder::Retrying> {
        if self.active.is_some() {
            return None;
        }
        let game = self.resume.as_ref()?;
        Some(crate::recorder::Retrying {
            game: game.clone(),
            reason: self.restarts.reason()?.to_owned(),
            retry_in: self.restarts.retry_in(),
        })
    }

    pub async fn save(&self, root: &Path, seconds: Option<u32>) -> Result<SavedClip> {
        let a = self.active.as_ref().context("no game is being recorded")?;
        a.recorder
            .save(seconds, &clip_path(root, &a.game.name))
            .await
    }

    /// Mark this moment in the current recording. Returns how far into
    /// the replay buffer it is, in seconds.
    pub fn mark(&self) -> Result<f64> {
        self.active
            .as_ref()
            .context("no game is being recorded")?
            .recorder
            .mark()
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
