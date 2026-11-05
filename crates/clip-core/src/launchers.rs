//! Games installed by launchers other than Steam, so they are recognised
//! by their executable's folder (even windowed) and named properly. Each
//! launcher keeps its own list of installs; this reads the ones it can:
//!
//! - Epic Games: `*.item` JSON manifests (`DisplayName`, `InstallLocation`).
//! - Riot: `RiotClientInstalls.json` (`associated_client` keys are game
//!   folders).
//! - Xbox / Game Pass (GDK): anything under an `XboxGames\<Name>` folder.
//! - GOG, Ubisoft Connect, EA, Battle.net (Windows): the registry, see
//!   [`windows_index`].
//!
//! Steam has its own index (library folders and app manifests).

use std::path::{Path, PathBuf};

/// A game folder and the game's name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub dir: PathBuf,
    pub name: String,
}

/// Install folders → names, looked up by an executable's path.
#[derive(Debug, Default)]
pub struct InstallIndex {
    /// Lower-cased folder with `/` separators and a trailing `/`, and name.
    dirs: Vec<(String, String)>,
}

impl InstallIndex {
    pub fn new(installs: impl IntoIterator<Item = Installed>) -> Self {
        let mut dirs: Vec<(String, String)> = installs
            .into_iter()
            .filter(|i| !i.name.trim().is_empty() && !i.dir.as_os_str().is_empty())
            .map(|i| (key(&i.dir), i.name.trim().to_owned()))
            // A drive root would claim everything on it.
            .filter(|(d, _)| d.matches('/').count() > 1)
            .collect();
        // Longest folder first, so nested installs win.
        dirs.sort_by_key(|d| std::cmp::Reverse(d.0.len()));
        dirs.dedup_by(|a, b| a.0 == b.0);
        Self { dirs }
    }

    pub fn len(&self) -> usize {
        self.dirs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.dirs.is_empty()
    }

    /// The game whose folder holds `exe`.
    pub fn lookup(&self, exe: &Path) -> Option<&str> {
        let exe = key(exe);
        self.dirs
            .iter()
            .find(|(dir, _)| exe.starts_with(dir.as_str()))
            .map(|(_, name)| name.as_str())
    }
}

/// Comparison key: lower case, `/` separators, trailing `/`.
fn key(p: &Path) -> String {
    let mut s = p.to_string_lossy().to_lowercase().replace('\\', "/");
    if !s.ends_with('/') {
        s.push('/');
    }
    s
}

/// Epic Games Launcher manifests in `dir` (normally
/// `%ProgramData%\Epic\EpicGamesLauncher\Data\Manifests`). Entries that say
/// they aren't games (plugins, engines) are left out.
pub fn epic_manifests(dir: &Path) -> Vec<Installed> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![];
    };
    entries
        .flatten()
        .filter(|e| {
            e.path()
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("item"))
        })
        .filter_map(|e| {
            let text = std::fs::read_to_string(e.path()).ok()?;
            let v: serde_json::Value = serde_json::from_str(&text).ok()?;
            if let Some(cats) = v.get("AppCategories").and_then(|c| c.as_array())
                && !cats.iter().any(|c| c.as_str() == Some("games"))
            {
                return None;
            }
            Some(Installed {
                dir: PathBuf::from(v.get("InstallLocation")?.as_str()?),
                name: v.get("DisplayName")?.as_str()?.to_owned(),
            })
        })
        .collect()
}

/// Riot games from `RiotClientInstalls.json` (normally in
/// `%ProgramData%\Riot Games`): its `associated_client` keys are game
/// folders such as `C:/Riot Games/VALORANT/live/`.
pub fn riot_installs(path: &Path) -> Vec<Installed> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return vec![];
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return vec![];
    };
    let Some(map) = v.get("associated_client").and_then(|m| m.as_object()) else {
        return vec![];
    };
    map.keys()
        .filter_map(|dir| {
            let p = PathBuf::from(dir);
            Some(Installed {
                name: riot_name(&p)?,
                dir: p,
            })
        })
        .collect()
}

/// `.../Riot Games/VALORANT/live/` → "VALORANT"; the folder above a
/// patchline folder (live, pbe), else the last folder.
fn riot_name(dir: &Path) -> Option<String> {
    let parts: Vec<String> = dir
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .filter(|c| !c.is_empty() && c != "/" && c != "\\")
        .collect();
    let last = parts.last()?;
    let name = if ["live", "pbe", "beta"].contains(&last.to_lowercase().as_str()) {
        parts.get(parts.len().checked_sub(2)?)?
    } else {
        last
    };
    Some(name.clone())
}

/// Xbox app / Game Pass (GDK) games live in `<drive>\XboxGames\<Name>\`.
/// The name is the game's own display name from its `MicrosoftGame.config`
/// when it can be read, else the folder's.
pub fn xbox_game(exe: &Path) -> Option<Installed> {
    let parts: Vec<_> = exe.components().collect();
    let i = parts
        .iter()
        .position(|c| c.as_os_str().eq_ignore_ascii_case("XboxGames"))?;
    let folder = parts.get(i + 1)?.as_os_str().to_string_lossy().into_owned();
    let dir: PathBuf = parts[..=i + 1].iter().collect();
    let config = dir.join("Content").join("MicrosoftGame.config");
    let name = std::fs::read_to_string(config)
        .ok()
        .and_then(|c| xml_attr(&c, "DefaultDisplayName"))
        .filter(|n| !n.starts_with("ms-resource:"))
        .unwrap_or(folder);
    Some(Installed { dir, name })
}

/// Part of Windows itself (under the Windows folder, or a built-in
/// "SystemApps" package such as the emoji panel): never a game, whatever
/// its windows look like.
pub fn is_windows_component(exe: &Path, windows_dir: &Path) -> bool {
    let norm = |p: &Path| p.to_string_lossy().replace('/', "\\").to_lowercase();
    let exe = norm(exe);
    let windir = norm(windows_dir);
    let windir = windir.trim_end_matches('\\');
    (!windir.is_empty() && exe.starts_with(&format!("{windir}\\")))
        || exe.contains("\\systemapps\\")
}

/// The first `attr="value"` in an XML text.
fn xml_attr(text: &str, attr: &str) -> Option<String> {
    let start = text.find(&format!("{attr}=\""))? + attr.len() + 2;
    let end = text[start..].find('"')? + start;
    Some(text[start..end].to_owned())
}

/// Every launcher's installs on this Windows machine: Epic and Riot from
/// their files under `%ProgramData%`, GOG, Ubisoft Connect, EA and
/// Battle.net from the registry.
#[cfg(windows)]
pub fn windows_index() -> InstallIndex {
    let program_data = std::env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:\\ProgramData"));
    let mut all = epic_manifests(&program_data.join("Epic\\EpicGamesLauncher\\Data\\Manifests"));
    all.extend(riot_installs(
        &program_data.join("Riot Games\\RiotClientInstalls.json"),
    ));
    all.extend(registry::gog());
    all.extend(registry::ubisoft());
    all.extend(registry::ea());
    all.extend(registry::uninstall_entries(|publisher, uninstall| {
        publisher.eq_ignore_ascii_case("Blizzard Entertainment")
            || uninstall.to_lowercase().contains("battle.net")
    }));
    InstallIndex::new(all)
}

/// Executables Windows itself counts as games on this PC (its Game Bar /
/// Game Mode list, `HKCU\System\GameConfigStore\Children`), lower-cased.
/// Undocumented and incomplete, so it adds games but is never the only
/// source; marking an app with Game Bar's "Remember this is a game" puts
/// it here.
#[cfg(windows)]
pub fn windows_known_games() -> std::collections::HashSet<String> {
    registry::known_games()
}

#[cfg(windows)]
mod registry {
    use std::path::PathBuf;

    use winreg::{
        RegKey,
        enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ},
    };

    use super::Installed;

    /// `GameConfigStore\Children\*\MatchedExeFullPath`, lower-cased.
    pub fn known_games() -> std::collections::HashSet<String> {
        let Ok(children) = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags("System\\GameConfigStore\\Children", KEY_READ)
        else {
            return Default::default();
        };
        children
            .enum_keys()
            .flatten()
            .filter_map(|k| children.open_subkey(k).ok())
            .filter_map(|k| k.get_value::<String, _>("MatchedExeFullPath").ok())
            .map(|p| p.to_lowercase())
            .collect()
    }

    fn open(path: &str) -> Option<RegKey> {
        RegKey::predef(HKEY_LOCAL_MACHINE)
            .open_subkey_with_flags(path, KEY_READ)
            .ok()
    }

    /// GOG Galaxy: `GOG.com\Games\<id>` with `path` and `gameName`.
    pub fn gog() -> Vec<Installed> {
        let Some(games) = open("SOFTWARE\\WOW6432Node\\GOG.com\\Games") else {
            return vec![];
        };
        games
            .enum_keys()
            .flatten()
            .filter_map(|id| {
                let k = games.open_subkey(id).ok()?;
                Some(Installed {
                    dir: PathBuf::from(k.get_value::<String, _>("path").ok()?),
                    name: k
                        .get_value::<String, _>("gameName")
                        .or_else(|_| k.get_value::<String, _>("GAMENAME"))
                        .ok()?,
                })
            })
            .collect()
    }

    /// Ubisoft Connect: `Ubisoft\Launcher\Installs\<id>\InstallDir`, named
    /// from its uninstall entry (`Uplay Install <id>`), else the folder.
    pub fn ubisoft() -> Vec<Installed> {
        let Some(installs) = open("SOFTWARE\\WOW6432Node\\Ubisoft\\Launcher\\Installs") else {
            return vec![];
        };
        let uninstall = open("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall");
        installs
            .enum_keys()
            .flatten()
            .filter_map(|id| {
                let dir = PathBuf::from(
                    installs
                        .open_subkey(&id)
                        .ok()?
                        .get_value::<String, _>("InstallDir")
                        .ok()?,
                );
                let name = uninstall
                    .as_ref()
                    .and_then(|u| u.open_subkey(format!("Uplay Install {id}")).ok())
                    .and_then(|k| k.get_value::<String, _>("DisplayName").ok())
                    .or_else(|| {
                        dir.components()
                            .next_back()
                            .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    })?;
                Some(Installed { dir, name })
            })
            .collect()
    }

    /// EA app and Origin: `EA Games\<name>` with `Install Dir`.
    pub fn ea() -> Vec<Installed> {
        let Some(games) = open("SOFTWARE\\WOW6432Node\\EA Games") else {
            return vec![];
        };
        games
            .enum_keys()
            .flatten()
            .filter_map(|name| {
                let k = games.open_subkey(&name).ok()?;
                let dir = k.get_value::<String, _>("Install Dir").ok()?;
                Some(Installed {
                    dir: PathBuf::from(dir),
                    name,
                })
            })
            .collect()
    }

    /// Uninstall entries (both registry views, machine and user) that
    /// `wanted(publisher, uninstall_string)` accepts and that say where
    /// they are installed.
    pub fn uninstall_entries(wanted: impl Fn(&str, &str) -> bool) -> Vec<Installed> {
        let roots = [
            (
                HKEY_LOCAL_MACHINE,
                "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
            ),
            (
                HKEY_LOCAL_MACHINE,
                "SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
            ),
            (
                HKEY_CURRENT_USER,
                "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
            ),
        ];
        let mut out = Vec::new();
        for (hive, path) in roots {
            let Ok(list) = RegKey::predef(hive).open_subkey_with_flags(path, KEY_READ) else {
                continue;
            };
            for id in list.enum_keys().flatten() {
                let Ok(k) = list.open_subkey(&id) else {
                    continue;
                };
                let publisher: String = k.get_value("Publisher").unwrap_or_default();
                let uninstall: String = k.get_value("UninstallString").unwrap_or_default();
                if !wanted(&publisher, &uninstall) {
                    continue;
                }
                let (Ok(name), Ok(dir)) = (
                    k.get_value::<String, _>("DisplayName"),
                    k.get_value::<String, _>("InstallLocation"),
                ) else {
                    continue;
                };
                // The launcher itself isn't a game.
                if name.to_lowercase().contains("battle.net") {
                    continue;
                }
                out.push(Installed {
                    dir: PathBuf::from(dir),
                    name,
                });
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_components_are_never_games() {
        let windir = Path::new("C:\\Windows");
        for exe in [
            "C:\\Windows\\SystemApps\\MicrosoftWindows.Client.CBS_cw5n1h2txyewy\\TextInputHost.exe",
            "c:\\windows\\system32\\SnippingTool.exe",
            "C:\\Windows\\explorer.exe",
        ] {
            assert!(is_windows_component(Path::new(exe), windir), "{exe}");
        }
        for exe in [
            "D:\\SteamLibrary\\steamapps\\common\\Overwatch\\Overwatch.exe",
            "C:\\Games\\Windowsill\\game.exe",
            "C:\\WindowsGames\\x.exe",
        ] {
            assert!(!is_windows_component(Path::new(exe), windir), "{exe}");
        }
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("reruns-launchers-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn index_finds_the_deepest_folder_holding_an_exe() {
        let idx = InstallIndex::new([
            Installed {
                dir: "D:\\Games\\Epic\\Fortnite".into(),
                name: "Fortnite".into(),
            },
            Installed {
                dir: "D:\\Games\\Epic\\Fortnite\\Tools".into(),
                name: "Fortnite tools".into(),
            },
            Installed {
                dir: "C:/Riot Games/VALORANT/live/".into(),
                name: "VALORANT".into(),
            },
            // Drive roots and empty names are ignored.
            Installed {
                dir: "D:\\".into(),
                name: "Everything".into(),
            },
            Installed {
                dir: "E:\\X".into(),
                name: " ".into(),
            },
        ]);
        assert_eq!(idx.len(), 3);
        let at = |p: &str| idx.lookup(Path::new(p));
        assert_eq!(
            at("d:\\games\\epic\\FORTNITE\\FortniteGame\\Binaries\\Win64\\FortniteClient.exe"),
            Some("Fortnite")
        );
        assert_eq!(
            at("D:\\Games\\Epic\\Fortnite\\Tools\\x.exe"),
            Some("Fortnite tools")
        );
        assert_eq!(
            at(
                "C:\\Riot Games\\VALORANT\\live\\ShooterGame\\Binaries\\Win64\\VALORANT-Win64-Shipping.exe"
            ),
            Some("VALORANT")
        );
        // A sibling folder with the same prefix is not a match.
        assert_eq!(at("D:\\Games\\Epic\\FortniteOld\\a.exe"), None);
        assert_eq!(at("D:\\Other\\a.exe"), None);
    }

    #[test]
    fn reads_epic_manifests_and_skips_non_games() {
        let d = tmp("epic");
        std::fs::write(
            d.join("A.item"),
            r#"{"DisplayName":"Rocket League","InstallLocation":"C:\\Epic\\rocketleague","AppCategories":["public","games","applications"]}"#,
        )
        .unwrap();
        std::fs::write(
            d.join("B.item"),
            r#"{"DisplayName":"Unreal Engine","InstallLocation":"C:\\Epic\\UE_5.4","AppCategories":["engines"]}"#,
        )
        .unwrap();
        std::fs::write(d.join("C.item"), "not json").unwrap();
        std::fs::write(d.join("notes.txt"), "{}").unwrap();
        let found = epic_manifests(&d);
        assert_eq!(
            found,
            [Installed {
                dir: "C:\\Epic\\rocketleague".into(),
                name: "Rocket League".into()
            }]
        );
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn reads_riot_installs() {
        let d = tmp("riot");
        let f = d.join("RiotClientInstalls.json");
        std::fs::write(
            &f,
            r#"{"associated_client":{"C:/Riot Games/VALORANT/live/":"C:/Riot Games/Riot Client/RiotClientServices.exe","C:/Riot Games/League of Legends/":"C:/Riot Games/Riot Client/RiotClientServices.exe"},"rc_default":"x"}"#,
        )
        .unwrap();
        let mut names: Vec<_> = riot_installs(&f).into_iter().map(|i| i.name).collect();
        names.sort();
        assert_eq!(names, ["League of Legends", "VALORANT"]);
        assert!(riot_installs(&d.join("missing.json")).is_empty());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn names_xbox_games_from_their_config_or_folder() {
        let d = tmp("xbox");
        let game = d.join("XboxGames").join("Forza Horizon 5");
        std::fs::create_dir_all(game.join("Content")).unwrap();
        std::fs::write(
            game.join("Content").join("MicrosoftGame.config"),
            r#"<Game><ShellVisuals DefaultDisplayName="Forza Horizon 5" PublisherDisplayName="Xbox Game Studios"/></Game>"#,
        )
        .unwrap();
        let exe = game.join("Content").join("ForzaHorizon5.exe");
        let found = xbox_game(&exe).unwrap();
        assert_eq!(found.name, "Forza Horizon 5");
        assert_eq!(found.dir, game);

        let bare = d
            .join("XboxGames")
            .join("Halo Infinite")
            .join("Content")
            .join("halo.exe");
        assert_eq!(xbox_game(&bare).unwrap().name, "Halo Infinite");
        assert!(xbox_game(Path::new("/games/halo.exe")).is_none());
        std::fs::remove_dir_all(&d).unwrap();
    }
}
