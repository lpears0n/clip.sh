//! Decide whether a window belongs to a game, and what it's called.

use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Game {
    /// Human-readable name, used for the clip folder.
    pub name: String,
    pub steam_app_id: Option<u32>,
}

/// Inspect the process behind a window. `title` is the fallback name.
pub fn detect(pid: u32, title: &str) -> Option<Game> {
    if pid == 0 {
        return None;
    }
    let proc = Path::new("/proc").join(pid.to_string());
    let environ = fs::read(proc.join("environ")).ok()?;
    let env = parse_environ(&environ);

    if let Some(id) = steam_app_id(&env) {
        let name = steam_app_name(id).unwrap_or_else(|| fallback_name(title, pid));
        return Some(Game {
            name,
            steam_app_id: Some(id),
        });
    }

    // Wine/Proton outside Steam (Lutris, Heroic, bottles...): a Windows exe.
    let cmdline = fs::read(proc.join("cmdline")).unwrap_or_default();
    if is_windows_exe(&cmdline) {
        return Some(Game {
            name: fallback_name(title, pid),
            steam_app_id: None,
        });
    }
    None
}

/// Every game running right now, found by scanning processes (no compositor
/// needed, so this works on any desktop). A game spawns many processes
/// (launcher, Proton, wineserver...); each game is reported once.
pub fn running_games() -> Vec<Game> {
    let mut games: Vec<Game> = Vec::new();
    let me = std::process::id();
    let Ok(entries) = fs::read_dir("/proc") else {
        return games;
    };
    for e in entries.flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else {
            continue;
        };
        if pid == me {
            continue;
        }
        // Other users' processes aren't readable; that's fine.
        let Ok(environ) = fs::read(e.path().join("environ")) else {
            continue;
        };
        let env = parse_environ(&environ);
        let game = if let Some(id) = steam_app_id(&env) {
            if games.iter().any(|g| g.steam_app_id == Some(id)) {
                continue;
            }
            Game {
                name: steam_app_name(id).unwrap_or_else(|| format!("Steam app {id}")),
                steam_app_id: Some(id),
            }
        } else {
            let cmdline = fs::read(e.path().join("cmdline")).unwrap_or_default();
            let Some(exe) = windows_exe(&cmdline) else {
                continue;
            };
            Game {
                name: exe,
                steam_app_id: None,
            }
        };
        if !games.iter().any(|g| g.name == game.name) {
            games.push(game);
        }
    }
    games
}

/// Steam (native and Proton) launches games with their app id set. Steam's
/// own processes don't have it, and 0 means "not a game".
fn steam_app_id(env: &[(&str, &str)]) -> Option<u32> {
    env_var(env, "SteamAppId")
        .or_else(|| env_var(env, "SteamGameId"))
        .and_then(|v| v.parse::<u32>().ok())
        .filter(|&id| id != 0)
}

/// The game's name from a Wine command line: the .exe's file stem.
fn windows_exe(cmdline: &[u8]) -> Option<String> {
    if !is_windows_exe(cmdline) {
        return None;
    }
    cmdline
        .split(|&b| b == 0)
        .filter_map(|a| std::str::from_utf8(a).ok())
        .find(|a| a.to_ascii_lowercase().ends_with(".exe"))
        .and_then(|a| a.rsplit(['\\', '/']).next())
        .and_then(|f| f.get(..f.len() - 4))
        .map(str::to_owned)
}

fn parse_environ(raw: &[u8]) -> Vec<(&str, &str)> {
    raw.split(|&b| b == 0)
        .filter_map(|kv| std::str::from_utf8(kv).ok()?.split_once('='))
        .collect()
}

fn env_var<'a>(env: &[(&str, &'a str)], key: &str) -> Option<&'a str> {
    env.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

fn is_windows_exe(cmdline: &[u8]) -> bool {
    cmdline
        .split(|&b| b == 0)
        .filter_map(|a| std::str::from_utf8(a).ok())
        .any(|a| {
            let a = a.to_ascii_lowercase();
            // Wine's own helpers aren't games.
            a.ends_with(".exe")
                && ![
                    "explorer.exe",
                    "winedevice.exe",
                    "services.exe",
                    "rpcss.exe",
                    "plugplay.exe",
                    "svchost.exe",
                    "steam.exe",
                    "steamwebhelper.exe",
                ]
                .iter()
                .any(|h| a.ends_with(h))
        })
}

fn fallback_name(title: &str, pid: u32) -> String {
    let t = title.trim();
    if !t.is_empty() {
        return t.to_owned();
    }
    fs::read_to_string(format!("/proc/{pid}/comm"))
        .map(|s| s.trim().to_owned())
        .unwrap_or_else(|_| "Game".into())
}

/// Look up an app's name in the Steam library manifests.
pub fn steam_app_name(id: u32) -> Option<String> {
    steam_libraries().into_iter().find_map(|lib| {
        let acf = fs::read_to_string(lib.join(format!("steamapps/appmanifest_{id}.acf"))).ok()?;
        vdf_value(&acf, "name")
    })
}

/// Steam's local artwork for an app (from the client's library cache).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct SteamArt {
    /// Portrait cover, 300×450.
    pub capsule: Option<PathBuf>,
    /// Wide banner, 1920×620.
    pub hero: Option<PathBuf>,
    /// Transparent logo, meant to sit on the hero.
    pub logo: Option<PathBuf>,
}

/// Find cached artwork for `id`. The client stores each image in a hashed
/// subfolder of `appcache/librarycache/<id>/`, so search by file name.
pub fn steam_art(id: u32) -> SteamArt {
    let mut art = SteamArt::default();
    for root in steam_roots() {
        let dir = root.join("appcache/librarycache").join(id.to_string());
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut files: Vec<PathBuf> = vec![];
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                if let Ok(inner) = fs::read_dir(&p) {
                    files.extend(inner.flatten().map(|f| f.path()));
                }
            } else {
                files.push(p);
            }
        }
        let find = |name: &str| {
            files
                .iter()
                .find(|f| f.file_name().is_some_and(|n| n == name))
                .cloned()
        };
        art.capsule = art
            .capsule
            .or_else(|| find("library_capsule.jpg").or_else(|| find("library_600x900.jpg")));
        art.hero = art.hero.or_else(|| find("library_hero.jpg"));
        art.logo = art.logo.or_else(|| find("logo.png"));
    }
    art
}

/// Folders holding Steam's own cache (not game libraries).
pub fn steam_roots() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return vec![];
    };
    let mut roots = Vec::new();
    for p in [
        home.join(".local/share/Steam"),
        home.join(".steam/steam"),
        home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
    ] {
        if let Ok(c) = p.canonicalize()
            && !roots.contains(&c)
        {
            roots.push(c);
        }
    }
    roots
}

/// Installed Steam apps as (id, name), from every library's manifests.
pub fn steam_apps() -> Vec<(u32, String)> {
    let mut apps = Vec::new();
    for lib in steam_libraries() {
        let Ok(entries) = fs::read_dir(lib.join("steamapps")) else {
            continue;
        };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Some(id) = name
                .strip_prefix("appmanifest_")
                .and_then(|n| n.strip_suffix(".acf"))
                .and_then(|n| n.parse::<u32>().ok())
            else {
                continue;
            };
            if let Ok(acf) = fs::read_to_string(e.path())
                && let Some(title) = vdf_value(&acf, "name")
                && !apps.iter().any(|(i, _)| *i == id)
            {
                apps.push((id, title));
            }
        }
    }
    apps
}

fn steam_libraries() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return vec![];
    };
    let roots = [
        home.join(".local/share/Steam"),
        home.join(".steam/steam"),
        home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
    ];
    let mut libs = Vec::new();
    for root in roots {
        let Ok(vdf) = fs::read_to_string(root.join("steamapps/libraryfolders.vdf")) else {
            continue;
        };
        for line in vdf.lines() {
            if let Some(p) = vdf_value(line, "path") {
                let p = PathBuf::from(p);
                if !libs.contains(&p) {
                    libs.push(p);
                }
            }
        }
        if !libs.contains(&root) {
            libs.push(root);
        }
    }
    libs
}

/// First `"key"  "value"` pair in a VDF/ACF text.
fn vdf_value(text: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    text.lines().find_map(|line| {
        let rest = line.trim().strip_prefix(&needle)?.trim();
        let v = rest.strip_prefix('"')?.strip_suffix('"')?;
        Some(v.replace("\\\\", "\\"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_vdf_values() {
        let acf = "\"AppState\"\n{\n\t\"appid\"\t\t\"730\"\n\t\"name\"\t\t\"Counter-Strike 2\"\n}";
        assert_eq!(vdf_value(acf, "name").as_deref(), Some("Counter-Strike 2"));
        assert_eq!(vdf_value(acf, "appid").as_deref(), Some("730"));
        assert_eq!(vdf_value(acf, "missing"), None);
    }

    #[test]
    fn recognises_windows_exes() {
        assert!(is_windows_exe(
            b"Z:\\games\\Elden Ring\\eldenring.exe\0-flag\0"
        ));
        assert!(!is_windows_exe(
            b"C:\\windows\\system32\\explorer.exe\0/desktop\0"
        ));
        assert!(!is_windows_exe(b"/usr/bin/konsole\0"));
    }

    #[test]
    fn environ_lookup() {
        let env = parse_environ(b"HOME=/home/x\0SteamAppId=730\0EMPTY=\0");
        assert_eq!(env_var(&env, "SteamAppId"), Some("730"));
        assert_eq!(env_var(&env, "EMPTY"), Some(""));
        assert_eq!(env_var(&env, "NOPE"), None);
    }

    #[test]
    fn finds_installed_apps_and_art() {
        // Only meaningful on a machine with Steam; just must not panic.
        let apps = steam_apps();
        if let Some((id, name)) = apps.first() {
            println!("{id} {name}: {:?}", steam_art(*id));
        }
    }

    #[test]
    fn names_wine_games_by_exe() {
        assert_eq!(
            windows_exe(b"Z:\\games\\Elden Ring\\eldenring.exe\0-flag\0").as_deref(),
            Some("eldenring")
        );
        assert_eq!(windows_exe(b"/usr/bin/konsole\0"), None);
    }

    #[test]
    fn scanning_finds_a_steam_game() {
        // A child with SteamAppId set must be reported as that game.
        let mut child = std::process::Command::new("sleep")
            .arg("5")
            .env("SteamAppId", "424242")
            .spawn()
            .unwrap();
        let games = running_games();
        child.kill().unwrap();
        let _ = child.wait();
        assert!(
            games.iter().any(|g| g.steam_app_id == Some(424242)),
            "{games:?}"
        );
    }

    #[test]
    fn own_process_is_not_a_game() {
        assert_eq!(detect(std::process::id(), "test"), None);
    }
}
