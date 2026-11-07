//! Updates from GitHub releases, through Tauri's updater: signed bundles,
//! checked against a public key built into the app.
//!
//! The updater is only switched on in builds that carry that key (release
//! builds once the owner has set it up; see docs/RELEASING.md). Other
//! builds report updates as not set up, and nothing is ever checked.
//!
//! Only the check runs by itself: one small request as the app starts
//! (whose result the app offers in a prompt) and then every six hours, so
//! nothing is downloaded while a game is being played. Downloading and
//! installing happen when the user asks, and end with a restart.

use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_updater::UpdaterExt;

use crate::AppState;

/// An update was found; the payload is an [`UpdateInfo`].
pub const UPDATE_AVAILABLE: &str = "update-available";
/// Download progress while installing, 0..=1 (`null` when the size is
/// unknown).
pub const UPDATE_PROGRESS: &str = "update-progress";

const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// After a failed check, as at login before the network is up: try again
/// this soon, then twice as long each time, up to [`CHECK_EVERY`].
const RETRY_FIRST: Duration = Duration::from_secs(60);

/// The newest update the timed checks found. The first check runs as the
/// app starts and can finish before the page listens for
/// [`UPDATE_AVAILABLE`], so the page also asks for it ([`found_update`]).
#[derive(Default)]
pub struct Found(Mutex<Option<UpdateInfo>>);

#[derive(Debug, Clone, Serialize)]
pub struct UpdateInfo {
    pub version: String,
    pub current_version: String,
    /// Release notes, as written on the release.
    pub notes: Option<String>,
    /// Found by the check at start-up: the app asks once whether to update.
    pub prompt: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateSupport {
    /// This build can update itself.
    pub configured: bool,
    pub version: String,
}

/// Whether `config` (as built) has an updater public key.
pub fn configured(config: &tauri::Config) -> bool {
    config
        .plugins
        .0
        .get("updater")
        .and_then(|u| u.get("pubkey"))
        .and_then(|k| k.as_str())
        .is_some_and(|k| !k.trim().is_empty())
}

/// The updater plugin, for builds that can use it.
pub fn plugin(
    config: &tauri::Config,
) -> Option<tauri::plugin::TauriPlugin<tauri::Wry, tauri_plugin_updater::Config>> {
    configured(config).then(|| tauri_plugin_updater::Builder::new().build())
}

/// Check by itself, at once and then on a timer, when this build can update
/// and the user hasn't turned it off.
pub fn spawn_checks(app: &AppHandle) {
    app.manage(Found::default());
    if !configured(app.config()) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // Until a check answers: what it finds is offered in a prompt.
        let mut starting = true;
        let mut failures = 0;
        loop {
            let enabled = app
                .state::<AppState>()
                .settings
                .lock()
                .unwrap()
                .check_for_updates;
            let mut wait = CHECK_EVERY;
            if !enabled {
                starting = false;
            } else {
                match check(&app).await {
                    Ok(update) => {
                        if let Some(mut info) = update {
                            info.prompt = starting;
                            announce(&app, info);
                        }
                        starting = false;
                        failures = 0;
                    }
                    Err(e) => {
                        tracing::info!("update check: {e}");
                        wait = retry_after(failures);
                        failures += 1;
                    }
                }
            }
            tokio::time::sleep(wait).await;
        }
    });
}

/// Keep `info` for the page and tell it, unless that version was already
/// announced.
fn announce(app: &AppHandle, info: UpdateInfo) {
    let found = app.state::<Found>();
    let mut found = found.0.lock().unwrap();
    if found.as_ref().is_some_and(|f| f.version == info.version) {
        return;
    }
    tracing::info!(version = %info.version, "update available");
    let _ = app.emit(UPDATE_AVAILABLE, &info);
    *found = Some(info);
}

/// How long to wait after the `failures`-th failed check in a row (from 0).
fn retry_after(failures: u32) -> Duration {
    RETRY_FIRST
        .saturating_mul(1 << failures.min(16))
        .min(CHECK_EVERY)
}

async fn check(app: &AppHandle) -> Result<Option<UpdateInfo>, String> {
    if !configured(app.config()) {
        return Err("updates aren't set up in this build".into());
    }
    let update = app
        .updater()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| e.to_string())?;
    Ok(update.map(|u| UpdateInfo {
        version: u.version.clone(),
        current_version: u.current_version.clone(),
        notes: u.body.clone().filter(|b| !b.trim().is_empty()),
        prompt: false,
    }))
}

#[tauri::command]
pub fn update_support(app: AppHandle) -> UpdateSupport {
    UpdateSupport {
        configured: configured(app.config()),
        version: app.package_info().version.to_string(),
    }
}

/// The update the timed checks found, if any.
#[tauri::command]
pub fn found_update(found: State<Found>) -> Option<UpdateInfo> {
    found.0.lock().unwrap().clone()
}

/// Look for a newer release now.
#[tauri::command]
pub async fn check_update(app: AppHandle) -> Result<Option<UpdateInfo>, String> {
    check(&app).await
}

/// Download and install the newest release, then restart into it. On
/// Windows the installer closes the app itself.
#[tauri::command]
pub async fn install_update(app: AppHandle) -> Result<(), String> {
    if !configured(app.config()) {
        return Err("updates aren't set up in this build".into());
    }
    let update = app
        .updater()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| e.to_string())?
        .ok_or("already up to date")?;
    tracing::info!(version = %update.version, "installing update");
    let progress = app.clone();
    let mut done = 0u64;
    update
        .download_and_install(
            move |chunk, total| {
                done += chunk as u64;
                let fraction = total.map(|t| (done as f64 / t.max(1) as f64).min(1.0));
                let _ = progress.emit(UPDATE_PROGRESS, fraction);
            },
            || {},
        )
        .await
        .map_err(|e| e.to_string())?;
    app.restart()
}

#[cfg(test)]
mod tests {
    use super::{CHECK_EVERY, configured, retry_after};
    use std::time::Duration;

    /// A failed check (no network yet at login) is tried again within a
    /// minute, then less often, never later than the usual check.
    #[test]
    fn a_failed_check_is_retried_soon() {
        let minutes = |f| retry_after(f).as_secs() / 60;
        assert_eq!([0, 1, 2, 3].map(minutes), [1, 2, 4, 8]);
        assert_eq!(retry_after(9), CHECK_EVERY);
        assert_eq!(retry_after(u32::MAX), CHECK_EVERY);
        assert!(retry_after(0) <= Duration::from_secs(60));
    }

    fn config(plugins: serde_json::Value) -> tauri::Config {
        let mut c: tauri::Config = serde_json::from_value(serde_json::json!({
            "identifier": "gg.reruns.test",
        }))
        .unwrap();
        c.plugins = serde_json::from_value(plugins).unwrap();
        c
    }

    #[test]
    fn needs_a_public_key() {
        assert!(!configured(&config(serde_json::json!({}))));
        assert!(!configured(&config(
            serde_json::json!({ "updater": { "pubkey": "  ", "endpoints": [] } })
        )));
        assert!(configured(&config(
            serde_json::json!({ "updater": { "pubkey": "dW50cnVzdGVk", "endpoints": [] } })
        )));
    }

    /// The shape scripts/release-config.mjs writes is what the plugin reads.
    #[test]
    fn release_config_is_a_valid_updater_config() {
        let c: tauri_plugin_updater::Config = serde_json::from_value(serde_json::json!({
            "pubkey": "dW50cnVzdGVk",
            "endpoints": ["https://github.com/lpears0n/reruns/releases/latest/download/latest.json"],
            "windows": { "installMode": "passive" },
        }))
        .unwrap();
        assert_eq!(c.endpoints.len(), 1);
    }
}
