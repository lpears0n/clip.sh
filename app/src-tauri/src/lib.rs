mod games;
mod ipc_server;
mod library;
mod media_server;
mod service;
mod settings;
mod storage;
mod system;

use std::{
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use clip_core::{
    Codec, Encoder,
    export::Edit,
    hotkey::{Hotkey, HotkeyBackend},
};
use serde::Serialize;

use service::{Service, Status};
use settings::Settings;
use tauri::{
    AppHandle, Emitter, Listener, Manager, State, WindowEvent,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};
use tauri_plugin_opener::OpenerExt;

pub(crate) struct AppState {
    pub(crate) service: Service,
    settings: Mutex<Settings>,
    /// `None` when KGlobalAccel isn't available (not KDE).
    /// `None` until registered, and when no hotkey service exists.
    shortcut: tokio::sync::Mutex<Option<Hotkey>>,
    encoder: OnceLock<Encoder>,
    /// Which encoders handle which codecs; probing test-encodes, so do it once.
    encoders: tokio::sync::OnceCell<Vec<EncoderSupport>>,
    media: media_server::MediaServer,
}

/// Trimmer playback stats from `HTMLVideoElement.getVideoPlaybackQuality()`,
/// logged to diagnose stutter in the webview's video path.
#[tauri::command]
fn log_playback(path: PathBuf, total: u64, dropped: u64, corrupted: u64, seconds: f64) {
    tracing::info!(
        path = %path.display(),
        total,
        dropped,
        corrupted,
        seconds,
        fps = total as f64 / seconds.max(0.001),
        "trimmer playback"
    );
}

/// Portal capture: show the desktop's screen picker once.
#[tauri::command]
async fn choose_screen(state: State<'_, AppState>) -> CmdResult<()> {
    state.service.choose_screen().await
}

/// How the global hotkey is provided: "kglobalaccel", "portal", or null when
/// the desktop has neither (the UI then explains `clipsh save`).
#[tauri::command]
async fn hotkey_backend(state: State<'_, AppState>) -> CmdResult<Option<HotkeyBackend>> {
    Ok(state.shortcut.lock().await.as_ref().map(Hotkey::backend))
}

/// Recently played games with clips, sessions and Steam art.
#[tauri::command]
async fn list_games(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CmdResult<Vec<games::GameEntry>> {
    let status = state.service.status().await.ok();
    let dir = state.clips_dir();
    let list = tauri::async_runtime::spawn_blocking(move || {
        let clips = library::list(&dir);
        let playing = status
            .as_ref()
            .and_then(|s| Some((s.game.clone()?, s.steam_app_id, s.session_start_ms?)));
        games::list(
            &clips,
            playing.as_ref().map(|(n, id, t)| (n.as_str(), *id, *t)),
        )
    })
    .await
    .map_err(err)?;
    // Steam's art lives outside the clips folder.
    let scope = app.asset_protocol_scope();
    for g in &list {
        for p in [&g.art.capsule, &g.art.hero, &g.art.logo]
            .into_iter()
            .flatten()
        {
            let _ = scope.allow_file(p);
        }
    }
    Ok(list)
}

/// Base URL for `<video>` sources; append the percent-encoded file path.
#[tauri::command]
fn media_base(state: State<'_, AppState>) -> String {
    state.media.base_url.clone()
}

#[derive(Debug, Clone, Serialize)]
struct EncoderSupport {
    encoder: Encoder,
    codecs: Vec<Codec>,
}

/// What this machine can do, for the settings UI.
#[derive(Debug, Clone, Serialize)]
struct Capabilities {
    display_hz: Option<f64>,
    encoders: Vec<EncoderSupport>,
    mic_devices: Vec<clip_core::InputDevice>,
}

#[tauri::command]
async fn get_capabilities(state: State<'_, AppState>) -> CmdResult<Capabilities> {
    let encoders = state
        .encoders
        .get_or_init(|| async {
            tauri::async_runtime::spawn_blocking(|| {
                Encoder::ALL
                    .into_iter()
                    .map(|encoder| EncoderSupport {
                        encoder,
                        codecs: Codec::ALL
                            .into_iter()
                            .filter(|&c| encoder.works(c))
                            .collect(),
                    })
                    .collect()
            })
            .await
            .unwrap_or_default()
        })
        .await
        .clone();
    let (display_hz, mic_devices) = tauri::async_runtime::spawn_blocking(|| {
        (
            clip_core::display::max_refresh_hz().ok().flatten(),
            clip_core::input_devices(),
        )
    })
    .await
    .map_err(err)?;
    Ok(Capabilities {
        display_hz,
        encoders,
        mic_devices,
    })
}

impl AppState {
    pub(crate) fn clips_dir(&self) -> PathBuf {
        self.settings.lock().unwrap().clips_dir.clone()
    }
}

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[tauri::command]
async fn get_status(state: State<'_, AppState>) -> CmdResult<Status> {
    state.service.status().await
}

#[tauri::command]
async fn save_clip(state: State<'_, AppState>) -> CmdResult<PathBuf> {
    state.service.save(state.clips_dir()).await
}

#[tauri::command]
async fn list_clips(state: State<'_, AppState>) -> CmdResult<Vec<library::Clip>> {
    let dir = state.clips_dir();
    tauri::async_runtime::spawn_blocking(move || library::list(&dir))
        .await
        .map_err(err)
}

#[tauri::command]
async fn delete_clip(state: State<'_, AppState>, path: PathBuf) -> CmdResult<()> {
    let use_trash = state.settings.lock().unwrap().use_trash;
    library::delete(&state.clips_dir(), &path, use_trash).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
async fn get_storage(state: State<'_, AppState>) -> CmdResult<storage::Usage> {
    let (dir, limit) = {
        let s = state.settings.lock().unwrap();
        (s.clips_dir.clone(), s.storage_policy().limit_bytes)
    };
    tauri::async_runtime::spawn_blocking(move || storage::usage(&dir, limit))
        .await
        .map_err(err)
}

/// Emitted when clips disappear behind the UI's back (storage limit).
const LIBRARY_CHANGED: &str = "library-changed";

/// Apply the storage limit off the async runtime; tell the UI if anything
/// was removed.
fn enforce_storage(app: &AppHandle) {
    let (dir, policy) = {
        let state = app.state::<AppState>();
        let s = state.settings.lock().unwrap();
        (s.clips_dir.clone(), s.storage_policy())
    };
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || match storage::enforce(&dir, policy) {
        Ok(removed) if !removed.is_empty() => {
            let _ = app.emit(LIBRARY_CHANGED, removed.len());
        }
        Ok(_) => {}
        Err(e) => tracing::warn!("storage limit: {e:#}"),
    });
}

#[tauri::command]
fn reveal_clip(app: AppHandle, path: PathBuf) -> CmdResult<()> {
    app.opener().reveal_item_in_dir(path).map_err(err)
}

#[tauri::command]
fn open_clips_folder(app: AppHandle, state: State<'_, AppState>) -> CmdResult<()> {
    let dir = state.clips_dir();
    std::fs::create_dir_all(&dir).map_err(err)?;
    app.opener()
        .open_path(dir.to_string_lossy(), None::<&str>)
        .map_err(err)
}

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> Settings {
    state.settings.lock().unwrap().clone()
}

#[tauri::command]
async fn set_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: Settings,
) -> CmdResult<()> {
    let (old_hotkey, old_autostart) = {
        let s = state.settings.lock().unwrap();
        (s.hotkey.clone(), s.launch_at_login)
    };
    if settings.launch_at_login != old_autostart {
        system::set_autostart(settings.launch_at_login).map_err(|e| format!("{e:#}"))?;
    }
    if settings.hotkey != old_hotkey {
        match state.shortcut.lock().await.as_ref() {
            Some(s) => s
                .set_keys(&settings.hotkey)
                .await
                .map_err(|e| format!("{e:#}"))?,
            None => {
                return Err(
                    "your desktop has no global shortcut service; bind `clipsh save` in its keyboard settings"
                        .into(),
                );
            }
        }
    }
    settings.save().map_err(|e| format!("{e:#}"))?;
    allow_clips_dir(&app, &settings.clips_dir);
    state.service.configure(settings.recorder_config())?;
    *state.settings.lock().unwrap() = settings;
    enforce_storage(&app);
    Ok(())
}

/// Load any video for editing (also from outside the library).
#[tauri::command]
async fn open_for_edit(
    app: AppHandle,
    state: State<'_, AppState>,
    path: PathBuf,
) -> CmdResult<library::Clip> {
    app.asset_protocol_scope().allow_file(&path).map_err(err)?;
    state.media.allow(&path);
    tauri::async_runtime::spawn_blocking(move || {
        let game = path
            .parent()
            .and_then(Path::file_name)
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        library::load(&path, &game)
    })
    .await
    .map_err(err)?
    .map_err(|e| format!("{e:#}"))
}

/// Trim `path` to `start..end` seconds, mixing audio tracks at `volumes`.
#[tauri::command]
async fn render_clip(
    app: AppHandle,
    state: State<'_, AppState>,
    path: PathBuf,
    start: f64,
    end: f64,
    volumes: Vec<f64>,
) -> CmdResult<PathBuf> {
    let (out, cfg) = {
        let s = state.settings.lock().unwrap();
        (library::edit_path(&s.clips_dir, &path), s.video_config())
    };
    let encoder = match state.encoder.get() {
        Some(e) => *e,
        None => {
            let e = tauri::async_runtime::spawn_blocking(|| Encoder::probe(Codec::H264))
                .await
                .map_err(err)?
                .ok_or("no working H.264 encoder")?;
            *state.encoder.get_or_init(|| e)
        }
    };
    let edit = Edit {
        start: clip_core::gst::ClockTime::from_nseconds((start.max(0.0) * 1e9) as u64),
        end: clip_core::gst::ClockTime::from_nseconds((end.max(0.0) * 1e9) as u64),
        volumes,
    };
    let result = out.clone();
    let source = path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        clip_core::export::render(&path, &out, &edit, encoder, &cfg)
    })
    .await
    .map_err(err)?
    .map_err(|e| format!("{e:#}"))?;
    library::copy_meta(&source, &result);
    let _ = app.emit(service::CLIP_SAVED, &result);
    Ok(result)
}

/// Let the webview load videos and thumbnails through the asset protocol.
fn allow_clips_dir(app: &AppHandle, dir: &std::path::Path) {
    let scope = app.asset_protocol_scope();
    for d in [dir.to_owned(), library::cache_dir()] {
        let _ = std::fs::create_dir_all(&d);
        if let Err(e) = scope.allow_directory(&d, true) {
            tracing::warn!("asset scope for {}: {e}", d.display());
        }
    }
    if let Some(state) = app.try_state::<AppState>() {
        state.media.allow(dir);
    }
}

pub(crate) fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// Register the global hotkey and save a clip on every press.
async fn listen_hotkey(app: AppHandle, keys: String) {
    // CLIPSH_HOTKEY_PORTAL=1 tests the portal path on KDE.
    let skip_kde = std::env::var_os("CLIPSH_HOTKEY_PORTAL").is_some();
    let (shortcut, mut presses) = match Hotkey::register(&keys, skip_kde).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("global hotkey unavailable: {e:#}");
            return;
        }
    };
    let state = app.state::<AppState>();
    *state.shortcut.lock().await = Some(shortcut);
    while presses.recv().await.is_some() {
        if let Err(e) = state.service.save(state.clips_dir()).await {
            tracing::warn!("hotkey save failed: {e}");
        }
    }
}

/// Notification + sound for a newly saved clip, as configured.
fn announce_saved(app: &AppHandle, path: &std::path::Path) {
    let (notify, sound) = {
        let state = app.state::<AppState>();
        let s = state.settings.lock().unwrap();
        (s.notify_on_save, s.sound_on_save)
    };
    if sound {
        system::play_save_sound();
    }
    if notify {
        let game = path
            .parent()
            .and_then(std::path::Path::file_name)
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let summary = if library::is_edit(path) {
            "Edit rendered"
        } else {
            "Clip saved"
        };
        tauri::async_runtime::spawn(async move {
            if let Err(e) = system::notify(summary, &game).await {
                tracing::debug!("notify: {e:#}");
            }
        });
    }
}

fn setup_tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open clip.sh", true, None::<&str>)?;
    let save = MenuItem::with_id(app, "save", "Save clip", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &save, &quit])?;
    TrayIconBuilder::with_id("main")
        .icon(app.default_window_icon().unwrap().clone())
        .tooltip("clip.sh")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_main(app),
            "save" => {
                let state = app.state::<AppState>();
                let (service, dir) = (state.service.clone(), state.clips_dir());
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = service.save(dir).await {
                        tracing::warn!("save failed: {e}");
                    }
                });
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    if ipc_server::hand_off_to_running_instance() {
        tracing::info!("clip.sh is already running; showed its window");
        return;
    }
    if let Err(e) = clip_core::init() {
        panic!("GStreamer failed to initialise: {e}");
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        // Remember the window's size and position between launches. The
        // window starts hidden and VISIBLE is skipped so a tray launch stays
        // in the tray.
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::all()
                        - tauri_plugin_window_state::StateFlags::VISIBLE,
                )
                .build(),
        )
        .setup(|app| {
            let settings = Settings::load();
            let media = tauri::async_runtime::block_on(media_server::MediaServer::start())?;
            let token_path = dirs::data_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("clip.sh/portal-restore-token");
            let service = Service::spawn(
                app.handle().clone(),
                settings.recorder_config(),
                settings.capture_backend,
                token_path,
            );
            let hotkey = settings.hotkey.clone();
            app.manage(AppState {
                service,
                settings: Mutex::new(settings),
                shortcut: tokio::sync::Mutex::new(None),
                encoder: OnceLock::new(),
                encoders: tokio::sync::OnceCell::new(),
                media,
            });
            allow_clips_dir(app.handle(), &app.state::<AppState>().clips_dir());
            tauri::async_runtime::spawn(listen_hotkey(app.handle().clone(), hotkey));
            {
                let handle = app.handle().clone();
                app.listen_any(service::CLIP_SAVED, move |event| {
                    enforce_storage(&handle);
                    let path: PathBuf = serde_json::from_str(event.payload()).unwrap_or_default();
                    announce_saved(&handle, &path);
                });
            }
            // The window starts hidden (tauri.conf.json) so a login launch
            // can stay in the tray without flashing.
            let autostarted = std::env::args().any(|a| a == system::AUTOSTART_ARG);
            let in_tray = autostarted
                && app
                    .state::<AppState>()
                    .settings
                    .lock()
                    .unwrap()
                    .start_in_tray;
            if !in_tray {
                show_main(app.handle());
            }
            enforce_storage(app.handle());
            setup_tray(app.handle())?;
            if let Err(e) = ipc_server::serve(app.handle().clone()) {
                tracing::warn!("control socket unavailable: {e:#}");
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let to_tray = window
                    .app_handle()
                    .state::<AppState>()
                    .settings
                    .lock()
                    .unwrap()
                    .close_to_tray;
                if to_tray {
                    // Keep recording in the tray.
                    api.prevent_close();
                    let _ = window.hide();
                } else {
                    window.app_handle().exit(0);
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_status,
            save_clip,
            list_clips,
            delete_clip,
            get_storage,
            get_capabilities,
            list_games,
            choose_screen,
            hotkey_backend,
            media_base,
            log_playback,
            reveal_clip,
            open_clips_folder,
            get_settings,
            set_settings,
            open_for_edit,
            render_clip,
        ])
        .run(tauri::generate_context!())
        .expect("error while running clip.sh");
}
