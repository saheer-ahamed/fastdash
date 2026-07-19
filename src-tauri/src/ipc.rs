//! Tauri command surface the frontend calls.

use std::sync::{Arc, RwLock};

use tauri::{AppHandle, State};

use crate::engine::cache::SnapshotCache;
use crate::engine::config::AppConfig;
use crate::engine::connector::{ConnectorMeta, Snapshot};
use crate::engine::registry::Registry;
use crate::engine::{scheduler, secrets};

#[tauri::command]
pub fn list_connectors(registry: State<'_, Arc<Registry>>) -> Vec<ConnectorMeta> {
    registry.all().iter().map(|c| c.meta()).collect()
}

/// The latest cached snapshot for a connector, or `None` if the scheduler has
/// not fetched it yet. This is the fast path the UI reads first.
#[tauri::command]
pub fn get_cached(cache: State<'_, Arc<SnapshotCache>>, id: String) -> Option<Snapshot> {
    cache.get(&id)
}

/// Manually refresh one connector now. Fetches, updates the cache, emits
/// `connector:update`, and returns the fresh snapshot for the caller.
#[tauri::command]
pub async fn fetch_connector(
    app: AppHandle,
    registry: State<'_, Arc<Registry>>,
    cache: State<'_, Arc<SnapshotCache>>,
    config: State<'_, Arc<RwLock<AppConfig>>>,
    id: String,
) -> Result<Snapshot, String> {
    let connector = registry
        .get(&id)
        .ok_or_else(|| format!("unknown connector: {id}"))?;
    let timezone = config.read().unwrap().timezone.clone();
    let cache = Arc::clone(cache.inner());
    Ok(scheduler::refresh_one(&app, &connector, &cache, timezone).await)
}

#[tauri::command]
pub fn get_config(config: State<'_, Arc<RwLock<AppConfig>>>) -> AppConfig {
    config.read().unwrap().clone()
}

/// Persist a new config to disk and update the in-memory copy the scheduler and
/// commands read from.
#[tauri::command]
pub fn save_config(
    state: State<'_, Arc<RwLock<AppConfig>>>,
    config: AppConfig,
) -> Result<(), String> {
    crate::engine::config::save(&config).map_err(|e| e.to_string())?;
    // Apply the language immediately so connector panels localize on next fetch.
    crate::engine::i18n::set_locale(&config.locale);
    *state.write().unwrap() = config;
    Ok(())
}

#[tauri::command]
pub fn set_secret(connector: String, label: String, value: String) -> Result<(), String> {
    secrets::set(&connector, &label, &value).map_err(|e| e.to_string())
}

/// Whether a secret is already stored for `{connector}/{label}`. Lets the UI
/// show a "token stored" state without ever reading the secret back.
#[tauri::command]
pub fn has_secret(connector: String, label: String) -> bool {
    matches!(secrets::get(&connector, &label), Ok(Some(_)))
}

/// Start a GitHub Device Flow login: fetch a code pair and open the browser to
/// GitHub's verification page. The UI shows `userCode` while it awaits approval.
#[tauri::command]
pub async fn github_device_start(
) -> Result<crate::connectors::github::device_flow::DeviceCode, String> {
    crate::connectors::github::device_flow::start().await
}

/// Long-poll until the user approves the device login, then store the resulting
/// token in the keychain under `github/{label}` and return the account login.
#[tauri::command]
pub async fn github_device_poll(
    device_code: String,
    interval: u64,
    label: String,
) -> Result<String, String> {
    use crate::connectors::github::device_flow;
    let token = device_flow::poll(&device_code, interval).await?;
    let login = device_flow::fetch_login(&token).await?;
    secrets::set("github", &label, &token).map_err(|e| e.to_string())?;
    Ok(login)
}

#[tauri::command]
pub fn delete_secret(connector: String, label: String) -> Result<(), String> {
    secrets::delete(&connector, &label).map_err(|e| e.to_string())
}
