//! Find the game that is running: walk the top-level windows, look at the
//! process behind each, and decide from its executable. Cheap enough to run
//! every couple of seconds (a few hundred microseconds; no per-frame work).
//!
//! Only executables known to be games count: installed by Steam, another
//! launcher or the Xbox app, or on Windows' own list of games (Game Bar /
//! Game Mode). Being fullscreen proves nothing: the emoji panel, the
//! Snipping Tool and Game Bar all put screen-sized overlay windows up.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, RECT},
        Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromWindow},
        System::Threading::{
            OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
            QueryFullProcessImageNameW,
        },
        UI::WindowsAndMessaging::{
            EnumWindows, GWL_EXSTYLE, GetForegroundWindow, GetWindowLongPtrW, GetWindowRect,
            GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
            WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
        },
    },
    core::{BOOL, PWSTR},
};

use crate::{
    game::{Game, steam_app_name, steam_libraries, vdf_value},
    launchers::{self, InstallIndex},
};

/// A game with the window it is drawing into and the monitor that window
/// is on (what gets captured).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningGame {
    pub game: Game,
    pub hwnd: isize,
    pub monitor: isize,
    /// It's the window the user is interacting with.
    pub foreground: bool,
    /// The game's executable, lower-case file stem (`r5apex`), for the
    /// "not a game" list.
    pub exe: String,
}

/// Processes that own large windows but are never games.
const NOT_GAMES: &[&str] = &[
    "explorer",
    "dwm",
    "searchhost",
    "startmenuexperiencehost",
    "shellexperiencehost",
    "applicationframehost",
    "systemsettings",
    "taskmgr",
    "chrome",
    "msedge",
    "firefox",
    "brave",
    "opera",
    "vivaldi",
    "discord",
    "steam",
    "steamwebhelper",
    "epicgameslauncher",
    "galaxyclient",
    "ubisoftconnect",
    "upc",
    "battle.net",
    "origin",
    "eadesktop",
    "code",
    "devenv",
    "obs64",
    "obs32",
    "medal",
    "reruns",
    "vlc",
    "mpv",
    "wmplayer",
    "spotify",
    "slack",
    "teams",
    "zoom",
    "notepad",
    "cmd",
    "powershell",
    "windowsterminal",
    "wt",
    "conhost",
    // Overlays and capture tools that draw over games.
    "textinputhost",
    "screenclippinghost",
    "snippingtool",
    "gamebar",
    "gamebarftserver",
    "gameoverlayui",
    "gameoverlayui64",
    "nvidia overlay",
    "nvidia share",
    "nvcontainer",
    "radeonsoftware",
    "amdrsserv",
    "overwolf",
    "outplayed",
    "rtss",
    "msiafterburner",
    "msedgewebview2",
    "lockapp",
    "widgets",
];

/// How often the Steam library index is re-read (installs are rare).
const STEAM_REFRESH: Duration = Duration::from_secs(60);

/// Finds game windows, remembering between scans what it already looked
/// up: each process's executable (so a running game is opened once, not
/// every scan; anti-cheat inspects every handle opened to its process) and
/// the Steam library index (re-read once a minute instead of every scan).
#[derive(Default)]
pub struct Scanner {
    exes: HashMap<u32, Option<PathBuf>>,
    steam: Option<(SteamIndex, Instant)>,
    /// Other launchers' installs (Epic, Riot, GOG, Ubisoft, EA, Battle.net).
    launchers: Option<(InstallIndex, Instant)>,
    /// Xbox / Game Pass names by executable (they come from a file).
    xbox: HashMap<PathBuf, Option<String>>,
    /// Executables Windows lists as games (lower-cased full paths).
    known: HashSet<String>,
}

impl Scanner {
    /// Every game window right now; the foreground one first.
    pub fn scan(&mut self) -> Vec<RunningGame> {
        let mut windows: Vec<HWND> = Vec::new();
        // SAFETY: the callback only pushes handles into the Vec passed as LPARAM.
        let _ = unsafe { EnumWindows(Some(collect), LPARAM(&mut windows as *mut _ as isize)) };
        // SAFETY: plain query.
        let fg = unsafe { GetForegroundWindow() };
        if self
            .steam
            .as_ref()
            .is_none_or(|(_, at)| at.elapsed() > STEAM_REFRESH)
        {
            self.steam = Some((SteamIndex::load(), Instant::now()));
            let index = launchers::windows_index();
            tracing::debug!(games = index.len(), "launcher installs indexed");
            self.launchers = Some((index, Instant::now()));
            self.known = launchers::windows_known_games();
        }
        let steam = &self.steam.as_ref().unwrap().0;
        let installs = &self.launchers.as_ref().unwrap().0;
        let known = &self.known;
        let windows_dir = std::env::var_os("SystemRoot")
            .or_else(|| std::env::var_os("windir"))
            .map_or_else(|| PathBuf::from("C:\\Windows"), PathBuf::from);
        let me = std::process::id();

        let mut seen = HashSet::new();
        // Each game with its best window so far and that window's area.
        let mut games: Vec<(RunningGame, i64)> = Vec::new();
        for hwnd in windows {
            // SAFETY: hwnd came from EnumWindows moments ago; stale handles just fail.
            let visible = unsafe { IsWindowVisible(hwnd) }.as_bool();
            if !visible {
                continue;
            }
            let title = window_title(hwnd);
            if title.is_empty() || is_overlay(hwnd) {
                continue;
            }
            let mut pid = 0u32;
            // SAFETY: writes the owning process id into `pid`.
            unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
            if pid == 0 || pid == me {
                continue;
            }
            seen.insert(pid);
            let Some(exe) = self
                .exes
                .entry(pid)
                .or_insert_with(|| process_image(pid))
                .clone()
            else {
                continue;
            };
            let mut exe = exe;
            if stem_of(&exe) == "applicationframehost" {
                // A Microsoft Store (UWP) app: the frame window belongs to
                // the host; the app runs in the process of a child window.
                // The frame is still what gets captured.
                let Some(hosted) = hosted_process(hwnd, pid) else {
                    continue;
                };
                seen.insert(hosted);
                match self
                    .exes
                    .entry(hosted)
                    .or_insert_with(|| process_image(hosted))
                    .clone()
                {
                    Some(e) => exe = e,
                    None => continue,
                }
            }
            let stem = stem_of(&exe);
            if NOT_GAMES.contains(&stem.as_str())
                || launchers::is_windows_component(&exe, &windows_dir)
            {
                continue;
            }
            let xbox = self
                .xbox
                .entry(exe.clone())
                .or_insert_with(|| launchers::xbox_game(&exe).map(|g| g.name))
                .clone();
            let game = if let Some((id, name)) = steam.lookup(&exe) {
                Game {
                    name,
                    steam_app_id: Some(id),
                }
            } else if let Some(name) = installs.lookup(&exe).map(str::to_owned).or(xbox) {
                // Installed by a launcher: a game even when windowed.
                Game {
                    name,
                    steam_app_id: None,
                }
            } else if known.contains(&exe.to_string_lossy().to_lowercase()) {
                // On Windows' own list of games.
                Game {
                    name: title_or_stem(&title, &stem),
                    steam_app_id: None,
                }
            } else {
                // Not known to be a game: fullscreen or not, it isn't one.
                continue;
            };
            // SAFETY: plain query on a live window handle.
            let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
            let found = RunningGame {
                game,
                hwnd: hwnd.0 as isize,
                monitor: monitor.0 as isize,
                foreground: hwnd == fg,
                exe: stem,
            };
            let area = window_area(hwnd);
            // A game with several windows (launcher, splash, overlay) is
            // recorded through its largest, the same one every scan, so the
            // recording doesn't hop between them.
            match games.iter_mut().find(|(g, _)| g.game == found.game) {
                Some((g, best)) => {
                    let foreground = g.foreground || found.foreground;
                    if area > *best {
                        *g = found;
                        *best = area;
                    }
                    g.foreground = foreground;
                }
                None => games.push((found, area)),
            }
        }
        // Forget processes whose windows are gone (their ids get reused).
        self.exes.retain(|pid, _| seen.contains(pid));
        if self.xbox.len() > 256 {
            self.xbox.clear();
        }
        let mut games: Vec<RunningGame> = games.into_iter().map(|(g, _)| g).collect();
        games.sort_by_key(|g| !g.foreground);
        games
    }
}

fn stem_of(exe: &Path) -> String {
    exe.file_stem()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

/// The process of a UWP app hosted in `frame` (owned by `frame_pid`, the
/// ApplicationFrameHost): the owner of a child window that isn't the host.
fn hosted_process(frame: HWND, frame_pid: u32) -> Option<u32> {
    let mut children: Vec<HWND> = Vec::new();
    // SAFETY: the callback only pushes handles into the Vec passed as LPARAM.
    let _ = unsafe {
        windows::Win32::UI::WindowsAndMessaging::EnumChildWindows(
            Some(frame),
            Some(collect),
            LPARAM(&mut children as *mut _ as isize),
        )
    };
    children.into_iter().find_map(|child| {
        let mut pid = 0u32;
        // SAFETY: writes the owning process id into `pid`.
        unsafe { GetWindowThreadProcessId(child, Some(&mut pid)) };
        (pid != 0 && pid != frame_pid).then_some(pid)
    })
}

unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: lparam is the Vec<HWND> passed by `Scanner::scan` or
    // `hosted_process`.
    let list = unsafe { &mut *(lparam.0 as *mut Vec<HWND>) };
    list.push(hwnd);
    BOOL(1)
}

fn window_title(hwnd: HWND) -> String {
    // SAFETY: plain queries; the buffer is sized from the reported length.
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; len as usize + 1];
        let n = GetWindowTextW(hwnd, &mut buf);
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }
}

/// Full path of the process's executable.
fn process_image(pid: u32) -> Option<PathBuf> {
    // SAFETY: limited-information handles are allowed for most processes;
    // the handle is closed when `h` drops (owned handle type).
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        let ok =
            QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len)
                .is_ok();
        let _ = windows::Win32::Foundation::CloseHandle(h);
        ok.then(|| PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
    }
}

/// The window's size in pixels (0 if it can't be read).
fn window_area(hwnd: HWND) -> i64 {
    let mut r = RECT::default();
    // SAFETY: plain query; a stale handle just fails.
    if unsafe { GetWindowRect(hwnd, &mut r) }.is_err() {
        return 0;
    }
    i64::from(r.right - r.left).max(0) * i64::from(r.bottom - r.top).max(0)
}

/// An overlay rather than an app window: click-through, a tool window, or
/// one that never takes focus. Games draw into ordinary windows; overlays
/// (the emoji panel, Game Bar, capture tools, FPS counters) use these.
fn is_overlay(hwnd: HWND) -> bool {
    // SAFETY: plain query; a stale handle reads as 0.
    let ex = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32;
    ex & (WS_EX_TRANSPARENT.0 | WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0) != 0
}

fn title_or_stem(title: &str, stem: &str) -> String {
    let t = title.trim();
    if t.is_empty() || t.len() > 60 {
        stem.to_owned()
    } else {
        t.to_owned()
    }
}

/// Steam install folders → (app id, name), so an exe under
/// `steamapps/common/<installdir>` resolves without reading the process
/// environment (which Windows doesn't expose cheaply).
struct SteamIndex {
    /// Canonical `steamapps/common/<installdir>` → app.
    dirs: HashMap<PathBuf, (u32, String)>,
}

impl SteamIndex {
    fn load() -> Self {
        let mut dirs = HashMap::new();
        for lib in steam_libraries() {
            let apps = lib.join("steamapps");
            let Ok(entries) = std::fs::read_dir(&apps) else {
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
                let Ok(acf) = std::fs::read_to_string(e.path()) else {
                    continue;
                };
                if let Some(installdir) = vdf_value(&acf, "installdir") {
                    let title = vdf_value(&acf, "name")
                        .or_else(|| steam_app_name(id))
                        .unwrap_or_else(|| installdir.clone());
                    let dir = apps.join("common").join(installdir);
                    dirs.insert(normalize(&dir), (id, title));
                }
            }
        }
        Self { dirs }
    }

    fn lookup(&self, exe: &Path) -> Option<(u32, String)> {
        let exe = normalize(exe);
        exe.ancestors().find_map(|dir| self.dirs.get(dir).cloned())
    }
}

/// Case-insensitive comparison key for Windows paths.
fn normalize(p: &Path) -> PathBuf {
    PathBuf::from(p.to_string_lossy().to_ascii_lowercase().replace('/', "\\"))
}
