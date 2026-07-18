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

#[tauri::command]
pub fn delete_secret(connector: String, label: String) -> Result<(), String> {
    secrets::delete(&connector, &label).map_err(|e| e.to_string())
}
