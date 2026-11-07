//! Game icons for the Discord status: the ones Discord itself shows for
//! the games it detects. Its list of them (about 25,000 games with their
//! icons, Steam ids and executables) is 12.6 MB, so it is fetched at most
//! once a week, only while the status is on, and kept on disk in a compact
//! form. Discord loads the icon from its own CDN. Everything here is
//! async, run in a task of its own, so a slow network never holds up the
//! status.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

const LIST: &str = "https://discord.com/api/v10/applications/detectable";
const MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// What is known about the recorded game.
pub struct Game {
    pub name: String,
    pub steam_app_id: Option<u32>,
    /// The executable's file stem, any case.
    pub exe: Option<String>,
}

/// One game in Discord's list, as kept on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Known {
    id: String,
    icon: String,
    /// Name and aliases, [`key`]ed.
    names: Vec<String>,
    /// Executable file stems, lower-case.
    exes: Vec<String>,
    steam: Vec<u32>,
}

/// URL of Discord's icon for `game`, if Discord knows the game. May
/// download the list.
pub async fn icon_for(game: &Game) -> Option<String> {
    let known = known().await?;
    find(&known, game).map(|k| cdn(&k.id, &k.icon, 512))
}

/// URL of the icon of the Discord application `app_id` (Reruns' own, set
/// in the Developer Portal), for the badge in the corner.
pub async fn app_icon(app_id: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct App {
        icon: Option<String>,
    }
    let body = get(&format!(
        "https://discord.com/api/v10/applications/{app_id}/rpc"
    ))
    .await
    .map_err(|e| tracing::debug!("discord app icon: {e}"))
    .ok()?;
    let app: App = serde_json::from_slice(&body).ok()?;
    Some(cdn(app_id, &app.icon?, 128))
}

fn cdn(app_id: &str, icon: &str, size: u32) -> String {
    format!("https://cdn.discordapp.com/app-icons/{app_id}/{icon}.png?size={size}")
}

/// The game Discord knows as `game`: by Steam id, else by name, else by
/// executable when that points at one game (or one whose name starts like
/// ours: "Overwatch®" is "Overwatch 2" there).
fn find<'a>(known: &'a [Known], game: &Game) -> Option<&'a Known> {
    if let Some(id) = game.steam_app_id
        && let Some(k) = known.iter().find(|k| k.steam.contains(&id))
    {
        return Some(k);
    }
    let name = key(&game.name);
    if !name.is_empty()
        && let Some(k) = known.iter().find(|k| k.names.contains(&name))
    {
        return Some(k);
    }
    let exe = game.exe.as_deref()?.to_lowercase();
    let hits: Vec<&Known> = known.iter().filter(|k| k.exes.contains(&exe)).collect();
    match hits[..] {
        [] => None,
        [one] => Some(one),
        _ if name.is_empty() => None,
        _ => hits
            .into_iter()
            .filter(|k| k.names.iter().any(|n| n.starts_with(&name)))
            // The newest application: a sequel over the original.
            .max_by_key(|k| k.id.parse::<u64>().unwrap_or(0)),
    }
}

/// A name for comparing: lower-case letters and digits only, so
/// "Call of Duty®" and "call of duty" match.
fn key(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn cache_file() -> Option<PathBuf> {
    let app_dir = if cfg!(windows) {
        "gg.reruns.app"
    } else {
        "reruns"
    };
    Some(dirs::cache_dir()?.join(app_dir).join("discord-games.json"))
}

/// Discord's list, downloaded and condensed.
async fn download() -> Result<Vec<Known>, String> {
    let body = get(LIST).await?;
    tokio::task::spawn_blocking(move || parse(&body))
        .await
        .map_err(|e| e.to_string())?
}

async fn known() -> Option<Vec<Known>> {
    static CACHE: std::sync::OnceLock<Option<ListCache>> = std::sync::OnceLock::new();
    let cache = CACHE
        .get_or_init(|| cache_file().map(ListCache::new))
        .as_ref()?;
    cache.get(download).await
}

/// The list on disk.
struct ListCache {
    path: PathBuf,
    /// A download was attempted this run (at most one).
    tried: std::sync::atomic::AtomicBool,
    /// One load at a time: a lookup that comes while another downloads
    /// (the user switched games) waits for it and reads what it wrote,
    /// rather than finding no copy yet and calling the game unknown.
    loading: tokio::sync::Mutex<()>,
}

impl ListCache {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            tried: std::sync::atomic::AtomicBool::new(false),
            loading: tokio::sync::Mutex::new(()),
        }
    }

    /// The list: from disk while under a week old, else from `fetch`
    /// (once per run at most); a stale copy beats none. Parsing runs off
    /// the async threads.
    async fn get<F, Fut>(&self, fetch: F) -> Option<Vec<Known>>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Vec<Known>, String>>,
    {
        let _one_at_a_time = self.loading.lock().await;
        let fresh = tokio::fs::metadata(&self.path)
            .await
            .and_then(|m| m.modified())
            .is_ok_and(|t| t.elapsed().is_ok_and(|age| age < MAX_AGE));
        if !fresh && !self.tried.swap(true, std::sync::atomic::Ordering::Relaxed) {
            match fetch().await {
                Ok(list) => {
                    if let Some(dir) = self.path.parent() {
                        let _ = tokio::fs::create_dir_all(dir).await;
                    }
                    if let Ok(json) = serde_json::to_vec(&list) {
                        let _ = tokio::fs::write(&self.path, json).await;
                    }
                    return Some(list);
                }
                Err(e) => tracing::info!("discord game list: {e}"),
            }
        }
        let bytes = tokio::fs::read(&self.path).await.ok()?;
        tokio::task::spawn_blocking(move || serde_json::from_slice(&bytes).ok())
            .await
            .ok()
            .flatten()
    }
}

/// Discord's list, down to the games with an icon and what finds them.
fn parse(body: &[u8]) -> Result<Vec<Known>, String> {
    #[derive(Deserialize)]
    struct Detectable {
        id: String,
        name: String,
        #[serde(default)]
        aliases: Vec<String>,
        icon_hash: Option<String>,
        #[serde(default)]
        executables: Vec<Exe>,
        #[serde(default)]
        third_party_skus: Vec<Sku>,
    }
    #[derive(Deserialize)]
    struct Exe {
        name: String,
    }
    #[derive(Deserialize)]
    struct Sku {
        distributor: Option<String>,
        id: Option<String>,
    }
    let all: Vec<Detectable> = serde_json::from_slice(body).map_err(|e| e.to_string())?;
    Ok(all
        .into_iter()
        .filter_map(|d| {
            let icon = d.icon_hash.filter(|h| !h.is_empty())?;
            Some(Known {
                names: std::iter::once(&d.name)
                    .chain(&d.aliases)
                    .map(|n| key(n))
                    .filter(|n| !n.is_empty())
                    .collect(),
                // "win64/deadlock.exe" -> "deadlock"
                exes: d
                    .executables
                    .iter()
                    .filter_map(|e| {
                        let file = e.name.rsplit(['/', '\\']).next()?;
                        let stem = file.strip_suffix(".exe").unwrap_or(file);
                        Some(stem.to_lowercase())
                    })
                    .collect(),
                steam: d
                    .third_party_skus
                    .iter()
                    .filter(|s| s.distributor.as_deref() == Some("steam"))
                    .filter_map(|s| s.id.as_deref()?.parse().ok())
                    .collect(),
                id: d.id,
                icon,
            })
        })
        .collect())
}

async fn get(url: &str) -> Result<Vec<u8>, String> {
    // The updater's TLS setup: rustls with ring, installed once.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = reqwest::Client::builder()
        .user_agent(concat!("Reruns/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;
    let response = client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| e.to_string())?;
    Ok(response.bytes().await.map_err(|e| e.to_string())?.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known(id: &str, names: &[&str], exes: &[&str], steam: &[u32]) -> Known {
        Known {
            id: id.into(),
            icon: format!("icon{id}"),
            names: names.iter().map(|n| key(n)).collect(),
            exes: exes.iter().map(|e| e.to_string()).collect(),
            steam: steam.to_vec(),
        }
    }

    fn list() -> Vec<Known> {
        vec![
            known("1", &["Deadlock"], &["deadlock", "project8"], &[1422450]),
            known("2", &["Overwatch Classic"], &["overwatch"], &[]),
            known("3", &["Overwatch 2"], &["overwatch"], &[2357570]),
            known("4", &["Call of Duty®"], &["cod"], &[]),
            known("5", &["Some Game"], &["game"], &[]),
            known("6", &["Other Game"], &["game"], &[]),
        ]
    }

    fn found(name: &str, steam: Option<u32>, exe: Option<&str>) -> Option<String> {
        let list = list();
        find(
            &list,
            &Game {
                name: name.into(),
                steam_app_id: steam,
                exe: exe.map(Into::into),
            },
        )
        .map(|k| k.id.clone())
    }

    #[test]
    fn finds_the_game_discord_knows() {
        assert_eq!(
            found("Deadlock", Some(1422450), None).as_deref(),
            Some("1"),
            "by Steam id"
        );
        assert_eq!(
            found("call of duty", None, None).as_deref(),
            Some("4"),
            "by name, symbols aside"
        );
        assert_eq!(
            found("Unknown", None, Some("project8")).as_deref(),
            Some("1"),
            "an exe only one game has"
        );
        assert_eq!(
            found("Overwatch®", None, Some("Overwatch")).as_deref(),
            Some("3"),
            "of the games sharing its exe, the newest whose name starts like ours"
        );
        assert_eq!(
            found("Mystery", None, Some("game")),
            None,
            "a generic exe matches nothing"
        );
        assert_eq!(found("Mystery", None, None), None);
    }

    /// A second lookup while the first downloads (the user switched games)
    /// waits for that download and gets the list, rather than no list.
    #[tokio::test]
    async fn a_lookup_during_the_download_waits_for_it() {
        let dir = std::env::temp_dir().join(format!("reruns-list-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cache = ListCache::new(dir.join("discord-games.json"));
        let first = cache.get(|| async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            Ok(list())
        });
        let second = async {
            // Starts once the first has begun downloading.
            tokio::time::sleep(Duration::from_millis(10)).await;
            cache
                .get(|| async { panic!("downloaded twice in one run") })
                .await
        };
        let (a, b) = tokio::join!(first, second);
        assert_eq!(a.map(|l| l.len()), Some(list().len()));
        assert_eq!(
            b.map(|l| l.len()),
            Some(list().len()),
            "read what the first wrote"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn icon_urls_point_at_discords_cdn() {
        assert_eq!(
            cdn(
                "1276737795012165766",
                "08e679bae42c738a66c1b2e0e546ecc0",
                512
            ),
            "https://cdn.discordapp.com/app-icons/1276737795012165766/08e679bae42c738a66c1b2e0e546ecc0.png?size=512"
        );
    }
}
