//! Updates from GitHub releases, through Tauri's updater: signed bundles,
//! checked against a public key built into the app.
//!
//! The updater is only switched on in builds that carry that key (release
//! builds once the owner has set it up; see docs/RELEASING.md). Other
//! builds report updates as not set up, and nothing is ever checked.
//!
//! Only the check runs by itself: one small request a few minutes after
//! start and then every six hours, so nothing is downloaded while a game is
//! being played. Downloading and installing happen when the user asks, and
//! end with a restart.

use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::UpdaterExt;

use crate::AppState;

/// An update was found; the payload is an [`UpdateInfo`].
pub const UPDATE_AVAILABLE: &str = "update-available";
/// Download progress while installing, 0..=1 (`null` when the size is
/// unknown).
pub const UPDATE_PROGRESS: &str = "update-progress";

const FIRST_CHECK: Duration = Duration::from_secs(3 * 60);
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Debug, Clone, Serialize)]
pub struct UpdateInfo {
    pub version: String,
    pub current_version: String,
    /// Release notes, as written on the release.
    pub notes: Option<String>,
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

/// Check by itself on a timer, when this build can update and the user
/// hasn't turned it off.
pub fn spawn_checks(app: &AppHandle) {
    if !configured(app.config()) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK).await;
        let mut announced: Option<String> = None;
        loop {
            let enabled = app
                .state::<AppState>()
                .settings
                .lock()
                .unwrap()
                .check_for_updates;
            if enabled {
                match check(&app).await {
                    Ok(Some(info)) if announced.as_deref() != Some(info.version.as_str()) => {
                        tracing::info!(version = %info.version, "update available");
                        let _ = app.emit(UPDATE_AVAILABLE, &info);
                        announced = Some(info.version);
                    }
                    Ok(_) => {}
                    Err(e) => tracing::info!("update check: {e}"),
                }
            }
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
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
    }))
}

#[tauri::command]
pub fn update_support(app: AppHandle) -> UpdateSupport {
    UpdateSupport {
        configured: configured(app.config()),
        version: app.package_info().version.to_string(),
    }
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
    use super::configured;

    fn config(plugins: serde_json::Value) -> tauri::Config {
        let mut c: tauri::Config = serde_json::from_value(serde_json::json!({
            "identifier": "sh.clip.test",
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
            "endpoints": ["https://github.com/lpears0n/clip.sh/releases/latest/download/latest.json"],
            "windows": { "installMode": "passive" },
        }))
        .unwrap();
        assert_eq!(c.endpoints.len(), 1);
    }
}
