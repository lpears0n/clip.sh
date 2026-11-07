//! Discord Rich Presence: "<game> with Reruns / Clipping <game> / 3 clips
//! saved" on the user's Discord profile while a game is recorded. It talks
//! to the Discord app on this computer through its local IPC socket: no
//! login. While Discord isn't running nothing happens, and it is picked up
//! once it starts.
//!
//! The picture is Discord's own icon for the game (as Medal's status
//! shows), with Reruns' icon as a badge; see [`crate::discord_icons`]. A
//! game Discord doesn't know gets the Reruns icon alone.
//!
//! Discord shows one "Playing" card per computer, from the app that set it
//! last, so this one replaces its own "Playing <game>" (as Medal's does):
//! hence the game's name in the title.
//!
//! It runs as a task on the async runtime. Every exchange with Discord has
//! a time limit, and icons are looked up in tasks of their own that send
//! the result back, so neither a Discord that stops answering nor a slow
//! network holds up turning the status off or changing the game.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::io;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::time::{Instant, timeout, timeout_at};

/// The Discord application the status belongs to: its name ("Reruns") and
/// icon are what the profile shows. Set up in the Discord Developer Portal
/// by the owner; not a secret (every client sends it).
const APP_ID: &str = "1568480492167233636";

/// How often to look for Discord, and to resend the status (which notices
/// a Discord that restarted), while there is something to show.
const RETRY: Duration = Duration::from_secs(30);

/// Least time between two updates: Discord takes about five per 20 s, and
/// a burst of saves would go past that.
const MIN_GAP: Duration = Duration::from_secs(4);

/// Longest a read or write to Discord may take; past it the connection is
/// dropped and made again later.
const IO_LIMIT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq)]
pub struct Activity {
    pub game: String,
    pub steam_app_id: Option<u32>,
    /// The game's executable (file stem), for finding its icon.
    pub exe: Option<String>,
    /// When the play session began (ms since the epoch): Discord shows the
    /// time since.
    pub since_ms: Option<u64>,
}

enum Msg {
    Enabled(bool),
    Playing(Option<Activity>),
    ClipSaved,
    /// A lookup finished: Discord's icon for `game` (`None` when Discord
    /// doesn't know it), and Reruns' own if it was asked for.
    Icon {
        game: String,
        icon: Option<String>,
        badge: Option<String>,
    },
}

/// What the status should say.
#[derive(Default)]
struct Wanted {
    enabled: bool,
    playing: Option<Activity>,
    /// Clips saved since this play session began.
    clips: u32,
    /// Icons looked up so far, by game.
    icons: HashMap<String, Option<String>>,
    /// Reruns' own icon, for the badge on the game's.
    badge: Option<String>,
}

impl Wanted {
    fn apply(&mut self, m: Msg) {
        match m {
            Msg::Enabled(on) => self.enabled = on,
            // Status updates repeat the same game; a different game or a
            // new session of it starts the count again.
            Msg::Playing(a) => {
                if a != self.playing {
                    self.clips = 0;
                }
                self.playing = a;
            }
            Msg::ClipSaved => {
                if self.playing.is_some() {
                    self.clips += 1;
                }
            }
            Msg::Icon { game, icon, badge } => {
                self.icons.insert(game, icon);
                if badge.is_some() {
                    self.badge = badge;
                }
            }
        }
    }

    fn activity(&self) -> Option<Value> {
        let a = self.playing.as_ref().filter(|_| self.enabled)?;
        let icon = self.icons.get(&a.game).cloned().flatten();
        Some(activity_json(
            a,
            self.clips,
            icon.as_deref(),
            self.badge.as_deref(),
        ))
    }
}

/// The status shown on Discord, kept by a task of its own.
pub struct Presence {
    tx: UnboundedSender<Msg>,
}

impl Presence {
    pub fn start(enabled: bool) -> Self {
        let (tx, rx) = unbounded_channel();
        let _ = tx.send(Msg::Enabled(enabled));
        tauri::async_runtime::spawn(run(rx, tx.clone()));
        Self { tx }
    }

    /// The user turned the status on or off.
    pub fn set_enabled(&self, on: bool) {
        let _ = self.tx.send(Msg::Enabled(on));
    }

    /// The game being recorded, or none.
    pub fn set_playing(&self, activity: Option<Activity>) {
        let _ = self.tx.send(Msg::Playing(activity));
    }

    /// A clip of the game being recorded was saved.
    pub fn clip_saved(&self) {
        let _ = self.tx.send(Msg::ClipSaved);
    }
}

async fn run(mut rx: UnboundedReceiver<Msg>, tx: UnboundedSender<Msg>) {
    let mut wanted = Wanted::default();
    let mut link = Link::default();
    // Games whose icon has been asked for.
    let mut looking = HashSet::new();
    loop {
        // Nothing to show: let go of Discord, which clears the status.
        if wanted.activity().is_none() {
            link.close();
        }
        let wait = if wanted.activity().is_some() {
            RETRY
        } else {
            Duration::from_secs(24 * 60 * 60)
        };
        match timeout(wait, rx.recv()).await {
            Ok(Some(m)) => wanted.apply(m),
            Ok(None) => return,
            // Resend now and then: notices a Discord that restarted.
            Err(_) => link.sent = None,
        }
        // Gather what else arrives before the next update may go out.
        while let Ok(m) = timeout_at(link.last_send + MIN_GAP, rx.recv()).await {
            match m {
                Some(m) => wanted.apply(m),
                None => return,
            }
        }
        if let Some(activity) = wanted.activity() {
            link.send(activity).await;
        }
        // A game not looked up yet, now that its status shows: find its
        // icon in a task of its own (the first time, that downloads
        // Discord's list); the result comes back as a message.
        if let Some(a) = wanted.playing.as_ref().filter(|_| wanted.enabled)
            && link.conn.is_some()
            && !wanted.icons.contains_key(&a.game)
            && looking.insert(a.game.clone())
        {
            let game = crate::discord_icons::Game {
                name: a.game.clone(),
                steam_app_id: a.steam_app_id,
                exe: a.exe.clone(),
            };
            let need_badge = wanted.badge.is_none();
            let tx = tx.clone();
            tauri::async_runtime::spawn(async move {
                let icon = crate::discord_icons::icon_for(&game).await;
                let badge = match icon {
                    Some(_) if need_badge => crate::discord_icons::app_icon(APP_ID).await,
                    _ => None,
                };
                let _ = tx.send(Msg::Icon {
                    game: game.name,
                    icon,
                    badge,
                });
            });
        }
    }
}

/// The connection to Discord, and what it was last told.
struct Link {
    conn: Option<Conn<Pipe>>,
    /// So repeats aren't sent.
    sent: Option<Value>,
    last_send: Instant,
}

impl Default for Link {
    fn default() -> Self {
        Self {
            conn: None,
            sent: None,
            last_send: Instant::now(),
        }
    }
}

impl Link {
    fn close(&mut self) {
        self.conn = None;
        self.sent = None;
    }

    /// Show `activity`, connecting first if needed.
    async fn send(&mut self, activity: Value) {
        if self.sent.as_ref() == Some(&activity) {
            return;
        }
        if self.conn.is_none() {
            match Conn::open().await {
                Ok(c) => self.conn = Some(c),
                Err(e) => {
                    tracing::debug!("discord presence: {e}");
                    return;
                }
            }
        }
        let Some(c) = &mut self.conn else { return };
        self.last_send = Instant::now();
        match c.set_activity(Some(&activity)).await {
            Ok(()) => self.sent = Some(activity),
            // Discord answered but didn't take it: not worth a retry loop.
            Err(e) if e.kind() == io::ErrorKind::Other => {
                tracing::debug!("discord presence: {e}");
                self.sent = Some(activity);
            }
            // Gone, or not answering in time: connect again later.
            Err(e) => {
                tracing::debug!("discord presence: {e}");
                self.close();
            }
        }
    }
}

/// The second line: how many clips this session has.
fn clips_line(clips: u32) -> String {
    match clips {
        0 => "Ready to clip".into(),
        1 => "1 clip saved".into(),
        n => format!("{n} clips saved"),
    }
}

/// The activity Discord shows for `a`: the game's `icon` with Reruns'
/// `badge`, or without an icon Discord's default, Reruns' own icon.
fn activity_json(a: &Activity, clips: u32, icon: Option<&str>, badge: Option<&str>) -> Value {
    let mut v = json!({
        "name": format!("{} with Reruns", a.game),
        "details": format!("Clipping {}", a.game),
        "state": clips_line(clips),
    });
    if let Some(icon) = icon {
        let mut assets = json!({ "large_image": icon, "large_text": a.game });
        if let Some(badge) = badge {
            assets["small_image"] = json!(badge);
            assets["small_text"] = json!("Reruns");
        }
        v["assets"] = assets;
    }
    if let Some(ms) = a.since_ms {
        v["timestamps"] = json!({ "start": ms });
    }
    v
}

#[cfg(windows)]
type Pipe = tokio::net::windows::named_pipe::NamedPipeClient;
#[cfg(unix)]
type Pipe = tokio::net::UnixStream;

/// Where the Discord app listens: `discord-ipc-0` to `-9` (one per running
/// Discord: stable, PTB, Canary).
fn sockets() -> Vec<std::path::PathBuf> {
    #[cfg(windows)]
    {
        (0..10)
            .map(|i| format!(r"\\.\pipe\discord-ipc-{i}").into())
            .collect()
    }
    #[cfg(unix)]
    {
        let mut dirs: Vec<std::path::PathBuf> = ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"]
            .iter()
            .filter_map(std::env::var_os)
            .map(Into::into)
            .collect();
        dirs.push("/tmp".into());
        // Flatpak and Snap Discord put the socket in their own folder.
        let subs = ["", "app/com.discordapp.Discord", "snap.discord"];
        let mut out = Vec::new();
        for d in &dirs {
            for s in subs {
                for i in 0..10 {
                    out.push(d.join(s).join(format!("discord-ipc-{i}")));
                }
            }
        }
        out
    }
}

async fn connect(path: &std::path::Path) -> io::Result<Pipe> {
    #[cfg(windows)]
    {
        tokio::net::windows::named_pipe::ClientOptions::new().open(path)
    }
    #[cfg(unix)]
    {
        within(IO_LIMIT, tokio::net::UnixStream::connect(path)).await
    }
}

/// `io` within `limit`, else a timed-out error.
async fn within<T>(limit: Duration, io: impl Future<Output = io::Result<T>>) -> io::Result<T> {
    timeout(limit, io).await.unwrap_or_else(|_| {
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "Discord didn't answer in time",
        ))
    })
}

/// One connection to Discord: frames of `op: u32 LE, len: u32 LE, JSON`.
struct Conn<P> {
    pipe: P,
    nonce: u64,
    /// How long each read or write may take.
    limit: Duration,
}

const OP_HANDSHAKE: u32 = 0;
const OP_FRAME: u32 = 1;
const OP_CLOSE: u32 = 2;
const OP_PING: u32 = 3;
const OP_PONG: u32 = 4;

impl Conn<Pipe> {
    async fn open() -> io::Result<Self> {
        let mut last = io::Error::new(io::ErrorKind::NotFound, "Discord isn't running");
        for path in sockets() {
            match connect(&path).await {
                Ok(pipe) => {
                    let mut conn = Conn {
                        pipe,
                        nonce: 0,
                        limit: IO_LIMIT,
                    };
                    conn.handshake().await?;
                    return Ok(conn);
                }
                Err(e) => last = e,
            }
        }
        Err(last)
    }
}

impl<P: AsyncRead + AsyncWrite + Unpin> Conn<P> {
    async fn handshake(&mut self) -> io::Result<()> {
        self.send(OP_HANDSHAKE, &json!({ "v": 1, "client_id": APP_ID }))
            .await?;
        let reply = self.recv().await?;
        if reply["evt"] == "READY" {
            Ok(())
        } else {
            Err(refused(&reply))
        }
    }

    async fn set_activity(&mut self, activity: Option<&Value>) -> io::Result<()> {
        self.nonce += 1;
        let nonce = self.nonce.to_string();
        self.send(
            OP_FRAME,
            &json!({
                "cmd": "SET_ACTIVITY",
                "args": { "pid": std::process::id(), "activity": activity },
                "nonce": nonce,
            }),
        )
        .await?;
        let reply = self.recv().await?;
        if reply["evt"] == "ERROR" {
            Err(refused(&reply))
        } else {
            Ok(())
        }
    }

    async fn send(&mut self, op: u32, payload: &Value) -> io::Result<()> {
        let body = serde_json::to_vec(payload)?;
        let mut frame = Vec::with_capacity(8 + body.len());
        frame.extend(op.to_le_bytes());
        frame.extend((body.len() as u32).to_le_bytes());
        frame.extend(body);
        let pipe = &mut self.pipe;
        within(self.limit, async {
            pipe.write_all(&frame).await?;
            pipe.flush().await
        })
        .await
    }

    /// The next reply, answering pings on the way.
    async fn recv(&mut self) -> io::Result<Value> {
        loop {
            let pipe = &mut self.pipe;
            let (op, value) = within(self.limit, async {
                let mut head = [0u8; 8];
                pipe.read_exact(&mut head).await?;
                let op = u32::from_le_bytes([head[0], head[1], head[2], head[3]]);
                let len = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as usize;
                if len > 1 << 20 {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too big"));
                }
                let mut body = vec![0; len];
                pipe.read_exact(&mut body).await?;
                let value: Value = serde_json::from_slice(&body)?;
                Ok((op, value))
            })
            .await?;
            match op {
                OP_PING => self.send(OP_PONG, &value).await?,
                OP_CLOSE => return Err(refused(&value)),
                _ => return Ok(value),
            }
        }
    }
}

fn refused(reply: &Value) -> io::Error {
    let why = reply["data"]["message"]
        .as_str()
        .or_else(|| reply["message"].as_str())
        .unwrap_or("unexpected reply");
    io::Error::other(format!("Discord refused: {why}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use tokio::io::ReadBuf;

    /// A pretend Discord: replies queued up front (or none ever, when
    /// `silent`), what was sent recorded.
    struct Fake {
        replies: io::Cursor<Vec<u8>>,
        sent: Vec<u8>,
        silent: bool,
    }

    impl Fake {
        fn new(replies: &[(u32, Value)]) -> Self {
            let mut buf = Vec::new();
            for (op, v) in replies {
                let body = serde_json::to_vec(v).unwrap();
                buf.extend(op.to_le_bytes());
                buf.extend((body.len() as u32).to_le_bytes());
                buf.extend(body);
            }
            Self {
                replies: io::Cursor::new(buf),
                sent: Vec::new(),
                silent: false,
            }
        }

        /// The frames sent, decoded.
        fn frames(&self) -> Vec<(u32, Value)> {
            let mut out = Vec::new();
            let mut rest = &self.sent[..];
            while rest.len() >= 8 {
                let op = u32::from_le_bytes(rest[..4].try_into().unwrap());
                let len = u32::from_le_bytes(rest[4..8].try_into().unwrap()) as usize;
                out.push((op, serde_json::from_slice(&rest[8..8 + len]).unwrap()));
                rest = &rest[8 + len..];
            }
            out
        }
    }

    impl AsyncRead for Fake {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            if self.silent {
                return Poll::Pending;
            }
            let n = std::io::Read::read(&mut self.replies, buf.initialize_unfilled())?;
            buf.advance(n);
            Poll::Ready(Ok(()))
        }
    }

    impl AsyncWrite for Fake {
        fn poll_write(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            self.sent.extend_from_slice(buf);
            Poll::Ready(Ok(buf.len()))
        }
        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    fn conn(pipe: Fake) -> Conn<Fake> {
        Conn {
            pipe,
            nonce: 0,
            limit: Duration::from_millis(200),
        }
    }

    #[tokio::test]
    async fn shows_the_game_being_clipped() {
        let mut conn = conn(Fake::new(&[
            (OP_FRAME, json!({ "cmd": "DISPATCH", "evt": "READY" })),
            (OP_PING, json!({ "n": 1 })),
            (OP_FRAME, json!({ "cmd": "SET_ACTIVITY", "evt": null })),
        ]));
        conn.handshake().await.unwrap();
        let game = Activity {
            game: "Deadlock".into(),
            steam_app_id: Some(1422450),
            exe: Some("project8".into()),
            since_ms: Some(1_700_000_000_000),
        };
        conn.set_activity(Some(&activity_json(
            &game,
            3,
            Some("https://cdn/deadlock.png"),
            Some("https://cdn/reruns.png"),
        )))
        .await
        .unwrap();

        let frames = conn.pipe.frames();
        assert_eq!(frames[0].0, OP_HANDSHAKE);
        assert_eq!(frames[0].1["v"], 1);
        assert_eq!(frames[1].0, OP_FRAME);
        let args = &frames[1].1["args"];
        assert_eq!(frames[1].1["cmd"], "SET_ACTIVITY");
        assert_eq!(args["pid"], std::process::id());
        let a = &args["activity"];
        assert_eq!(a["name"], "Deadlock with Reruns");
        assert_eq!(a["details"], "Clipping Deadlock");
        assert_eq!(a["state"], "3 clips saved");
        assert_eq!(a["timestamps"]["start"], 1_700_000_000_000u64);
        assert_eq!(
            a["assets"],
            json!({
                "large_image": "https://cdn/deadlock.png",
                "large_text": "Deadlock",
                "small_image": "https://cdn/reruns.png",
                "small_text": "Reruns",
            })
        );
        assert!(a.get("buttons").is_none(), "no buttons yet");
        // The ping that arrived while waiting was answered with its payload.
        assert_eq!(frames[2], (OP_PONG, json!({ "n": 1 })));
    }

    /// A Discord that keeps the connection open but never answers times
    /// out instead of holding the status task forever.
    #[tokio::test]
    async fn a_silent_discord_times_out() {
        let mut fake = Fake::new(&[]);
        fake.silent = true;
        let mut conn = conn(fake);
        let started = Instant::now();
        let e = conn.handshake().await.unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    /// A game Discord doesn't know: no images, so Discord shows Reruns'
    /// own icon.
    #[test]
    fn before_the_first_clip_of_an_unknown_game() {
        let a = activity_json(
            &Activity {
                game: "Overwatch®".into(),
                steam_app_id: None,
                exe: None,
                since_ms: None,
            },
            0,
            None,
            Some("https://cdn/reruns.png"),
        );
        assert_eq!(a["name"], "Overwatch® with Reruns");
        assert_eq!(a["state"], "Ready to clip");
        assert!(a.get("assets").is_none());
        assert!(a.get("timestamps").is_none());
    }

    fn game(name: &str, since: u64) -> Activity {
        Activity {
            game: name.into(),
            steam_app_id: None,
            exe: None,
            since_ms: Some(since),
        }
    }

    /// Saves count per play session: repeated status for the same game
    /// keeps the count, another game or a new session starts over.
    #[test]
    fn clips_count_per_session() {
        let state = |w: &Wanted| w.activity().map(|a| a["state"].clone());
        let mut w = Wanted::default();
        w.apply(Msg::Enabled(true));
        w.apply(Msg::ClipSaved);
        assert_eq!(state(&w), None, "nothing shown while no game records");

        w.apply(Msg::Playing(Some(game("Deadlock", 1))));
        assert_eq!(state(&w), Some(json!("Ready to clip")));
        w.apply(Msg::ClipSaved);
        assert_eq!(state(&w), Some(json!("1 clip saved")));
        w.apply(Msg::Playing(Some(game("Deadlock", 1))));
        w.apply(Msg::ClipSaved);
        assert_eq!(state(&w), Some(json!("2 clips saved")));

        w.apply(Msg::Playing(Some(game("Deadlock", 2))));
        assert_eq!(state(&w), Some(json!("Ready to clip")), "a new session");
        w.apply(Msg::ClipSaved);
        w.apply(Msg::Playing(Some(game("FragPunk", 3))));
        assert_eq!(state(&w), Some(json!("Ready to clip")), "another game");

        w.apply(Msg::Enabled(false));
        assert_eq!(state(&w), None, "turned off");
    }

    /// A looked-up icon is the picture for its game only, with the badge.
    #[test]
    fn an_icon_arrives_for_its_game() {
        let assets = |w: &Wanted| w.activity().map(|a| a["assets"].clone());
        let mut w = Wanted::default();
        w.apply(Msg::Enabled(true));
        w.apply(Msg::Playing(Some(game("Deadlock", 1))));
        assert_eq!(assets(&w), Some(Value::Null), "no picture until found");
        w.apply(Msg::Icon {
            game: "Deadlock".into(),
            icon: Some("https://cdn/deadlock.png".into()),
            badge: Some("https://cdn/reruns.png".into()),
        });
        assert_eq!(
            assets(&w).unwrap()["large_image"],
            "https://cdn/deadlock.png"
        );
        assert_eq!(assets(&w).unwrap()["small_image"], "https://cdn/reruns.png");
        w.apply(Msg::Playing(Some(game("FragPunk", 2))));
        assert_eq!(assets(&w), Some(Value::Null), "not another game's");
    }

    #[tokio::test]
    async fn a_refusal_is_an_error() {
        let mut c = conn(Fake::new(&[(
            OP_CLOSE,
            json!({ "code": 4000, "message": "Invalid Client ID" }),
        )]));
        let e = c.handshake().await.unwrap_err();
        assert!(e.to_string().contains("Invalid Client ID"), "{e}");

        let mut c = conn(Fake::new(&[(
            OP_FRAME,
            json!({ "evt": "ERROR", "data": { "code": 4000, "message": "bad activity" } }),
        )]));
        let e = c.set_activity(None).await.unwrap_err();
        assert!(e.to_string().contains("bad activity"), "{e}");
    }
}
