//! Games: play sessions (logged by the recorder) joined with clips and Steam
//! artwork, for the "recently played" view.

use std::{
    collections::HashMap,
    io::Write,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Result;
use clip_core::game::{SteamArt, steam_apps, steam_art};
use serde::{Deserialize, Serialize};

use crate::library::Clip;

/// One stretch of playing a game, start to end.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub game: String,
    pub steam_app_id: Option<u32>,
    pub start_ms: u64,
    pub end_ms: u64,
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn sessions_file() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("clip.sh/sessions.jsonl")
}

/// Append a finished session to the log (one JSON object per line).
pub fn record(session: &Session) -> Result<()> {
    let path = sessions_file();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(f, "{}", serde_json::to_string(session)?)?;
    Ok(())
}

/// Every logged session, oldest first. Unreadable lines are skipped.
pub fn sessions() -> Vec<Session> {
    std::fs::read_to_string(sessions_file())
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

#[derive(Debug, Clone, Serialize)]
pub struct GameEntry {
    pub name: String,
    pub steam_app_id: Option<u32>,
    pub art: SteamArt,
    /// Newest clip's thumbnail, for games without Steam art.
    pub fallback_image: Option<PathBuf>,
    /// End of the last session (or now, while playing).
    pub last_played_ms: Option<u64>,
    /// Latest of last played and newest clip; the sort key.
    pub last_activity_ms: u64,
    pub total_play_ms: u64,
    pub clip_count: usize,
    pub clip_bytes: u64,
    /// Recent sessions, newest first (for "played 1h today").
    pub sessions: Vec<Session>,
    pub playing: bool,
}

impl GameEntry {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            steam_app_id: None,
            art: SteamArt::default(),
            fallback_image: None,
            last_played_ms: None,
            last_activity_ms: 0,
            total_play_ms: 0,
            clip_count: 0,
            clip_bytes: 0,
            sessions: vec![],
            playing: false,
        }
    }
}

/// Steam app id for a game name, when the clip predates metadata files.
fn id_by_name(apps: &[(u32, String)], name: &str) -> Option<u32> {
    let norm = |s: &str| {
        s.chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect::<String>()
    };
    let n = norm(name);
    apps.iter().find(|(_, t)| norm(t) == n).map(|(id, _)| *id)
}

/// Build the games list, most recently active first. `playing` is the game
/// being recorded right now and when its session started.
pub fn list(clips: &[Clip], playing: Option<(&str, Option<u32>, u64)>) -> Vec<GameEntry> {
    let apps = steam_apps();
    let mut games: HashMap<String, GameEntry> = HashMap::new();

    let mut all_sessions = sessions();
    if let Some((name, id, start)) = playing {
        all_sessions.push(Session {
            game: name.to_owned(),
            steam_app_id: id,
            start_ms: start,
            end_ms: now_ms(),
        });
    }
    for s in all_sessions.into_iter().rev() {
        let g = games
            .entry(s.game.clone())
            .or_insert_with(|| GameEntry::new(&s.game));
        g.steam_app_id = g.steam_app_id.or(s.steam_app_id);
        g.last_played_ms = Some(g.last_played_ms.unwrap_or(0).max(s.end_ms));
        g.total_play_ms += s.end_ms.saturating_sub(s.start_ms);
        if g.sessions.len() < 50 {
            g.sessions.push(s);
        }
    }

    // Clips arrive newest first, so the first one seen sets the fallback.
    for c in clips.iter().filter(|c| !c.is_edit) {
        let g = games
            .entry(c.game.clone())
            .or_insert_with(|| GameEntry::new(&c.game));
        g.clip_count += 1;
        g.clip_bytes += c.size_bytes;
        g.steam_app_id = g.steam_app_id.or(c.steam_app_id);
        if g.fallback_image.is_none() {
            g.fallback_image = c.thumbnail.clone();
        }
        g.last_activity_ms = g.last_activity_ms.max(c.created_ms);
    }

    let mut out: Vec<GameEntry> = games
        .into_values()
        .map(|mut g| {
            g.steam_app_id = g.steam_app_id.or_else(|| id_by_name(&apps, &g.name));
            if let Some(id) = g.steam_app_id {
                g.art = steam_art(id);
            }
            g.last_activity_ms = g.last_activity_ms.max(g.last_played_ms.unwrap_or(0));
            g.playing = playing.is_some_and(|(n, _, _)| n == g.name);
            g
        })
        .collect();
    out.sort_by_key(|g| {
        (
            std::cmp::Reverse(g.playing),
            std::cmp::Reverse(g.last_activity_ms),
        )
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_names_loosely() {
        let apps = vec![
            (381210, "Dead by Daylight".to_string()),
            (730, "Counter-Strike 2".to_string()),
        ];
        assert_eq!(id_by_name(&apps, "Dead by Daylight"), Some(381210));
        assert_eq!(id_by_name(&apps, "counter strike 2"), Some(730));
        assert_eq!(id_by_name(&apps, "Desktop"), None);
    }
}
