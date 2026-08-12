//! Tauri command surface the frontend calls.

use std::sync::{Arc, RwLock};

use serde::Serialize;
use tauri::{AppHandle, State};

use crate::engine::cache::SnapshotCache;
use crate::engine::config::AppConfig;
use crate::engine::connector::{ConnectorMeta, Snapshot};
use crate::engine::range::DateRange;
use crate::engine::registry::Registry;
use crate::engine::{refresh, secrets};

/// One connector as the sidebar sees it: its fixed identity plus whether it is
/// connected right now.
///
/// Flattened rather than a field on `ConnectorMeta`, because `meta()` takes no
/// config and cannot know the answer - it would have to invent a placeholder
/// that only this command overwrites, leaving every other `meta()` caller
/// reading a lie. The wire shape stays what it always was with one field added.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorEntry {
    #[serde(flatten)]
    pub meta: ConnectorMeta,
    pub configured: bool,
}

/// Every registered connector, each saying whether it is connected.
///
/// Async because `is_configured` reads the OS keychain and the filesystem and
/// this is on the path to first paint - a sync command would do that on the main
/// thread. The config is cloned out of the lock first: a std `RwLock` guard held
/// across an async body makes the future non-Send.
#[tauri::command]
pub async fn list_connectors(
    registry: State<'_, Arc<Registry>>,
    config: State<'_, Arc<RwLock<AppConfig>>>,
) -> Result<Vec<ConnectorEntry>, String> {
    let cfg = config.read().unwrap().clone();
    Ok(entries(registry.inner(), &cfg))
}

/// The list itself, split out so its invariants are testable without a Tauri
/// `State`. Nothing is filtered here: the sidebar decides what to show, while
/// `fetch_connector` and `github_fetch` still have to resolve a connector by id
/// after its credential is removed mid-session.
fn entries(registry: &Registry, cfg: &AppConfig) -> Vec<ConnectorEntry> {
    registry
        .all()
        .iter()
        .map(|c| ConnectorEntry {
            meta: c.meta(),
            configured: c.is_configured(cfg),
        })
        .collect()
}

/// The latest cached snapshot for a connector, or `None` if nothing has fetched
/// it yet this session. The UI reads this before fetching, so a dashboard opened
/// again paints instantly instead of flashing "Loading...".
#[tauri::command]
pub fn get_cached(cache: State<'_, Arc<SnapshotCache>>, id: String) -> Option<Snapshot> {
    cache.get(&id)
}

/// Fetch one connector now, over `range` (defaults to today): updates the cache,
/// emits `connector:update`, and returns the fresh snapshot for the caller.
///
/// The frontend is what schedules this - on opening a connector's tab, on its
/// cadence while that tab is on screen and the window has focus, and on the
/// Refresh button. Nothing fetches on its own in the background.
#[tauri::command]
pub async fn fetch_connector(
    app: AppHandle,
    registry: State<'_, Arc<Registry>>,
    cache: State<'_, Arc<SnapshotCache>>,
    config: State<'_, Arc<RwLock<AppConfig>>>,
    id: String,
    range: Option<DateRange>,
) -> Result<Snapshot, String> {
    let connector = registry
        .get(&id)
        .ok_or_else(|| format!("unknown connector: {id}"))?;
    let timezone = config.read().unwrap().timezone.clone();
    let cache = Arc::clone(cache.inner());
    let range = range.unwrap_or_default();
    Ok(refresh::refresh_one(&app, &connector, &cache, timezone, range).await)
}

#[tauri::command]
pub fn get_config(config: State<'_, Arc<RwLock<AppConfig>>>) -> AppConfig {
    config.read().unwrap().clone()
}

/// Persist a new config to disk and update the in-memory copy the commands read
/// from.
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

/// Connect the Claude connector to Anthropic Console with an Admin API key.
///
/// The key is verified against `/v1/organizations/me` **before** it is stored,
/// so a typo or a revoked key fails here with something the user can act on
/// rather than being written to the keychain and surfacing later as a broken
/// dashboard. Returns the organization name, which the caller persists to the
/// config so the dashboard can name it without re-querying.
///
/// There is deliberately no browser OAuth equivalent: Anthropic runs no
/// third-party OAuth client registration, and reserves subscription OAuth for
/// Claude Code and claude.ai. See `connectors::claude::admin_api`.
#[tauri::command]
pub async fn claude_connect(key: String) -> Result<String, String> {
    use crate::connectors::claude::{admin_api, CONSOLE_LABEL};

    let key = key.trim().to_string();
    if key.is_empty() {
        return Err("Paste an Admin API key first.".into());
    }

    let org = admin_api::verify_key(&key)
        .await
        .map_err(|e| e.to_string())?;
    secrets::set("claude", CONSOLE_LABEL, &key).map_err(|e| e.to_string())?;
    Ok(org.name)
}

/// Forget the stored Console key. The plan meters keep working afterwards -
/// they read Claude Code's own login on this machine, not this key.
#[tauri::command]
pub fn claude_disconnect() -> Result<(), String> {
    use crate::connectors::claude::CONSOLE_LABEL;
    secrets::delete("claude", CONSOLE_LABEL).map_err(|e| e.to_string())
}

/// Fetch the GitHub dashboard for one account, optionally scoped to a single org
/// (`org = None` means all of the account's orgs) and to a day range (`None`
/// means today). Drives the account sub-tabs, org filter, and date filter.
///
/// Only one GitHub fetch runs at a time: this cancels whatever was still in
/// flight, so flipping through sub-tabs or date ranges costs one fetch rather
/// than one per click. A superseded call comes back as `Err("superseded")`,
/// which the UI ignores. `force` skips the short reuse window behind the manual
/// Refresh button.
#[tauri::command]
pub async fn github_fetch(
    label: String,
    org: Option<String>,
    range: Option<DateRange>,
    force: Option<bool>,
) -> Result<Snapshot, String> {
    crate::connectors::github::fetch_account(
        label,
        org,
        range.unwrap_or_default(),
        force.unwrap_or(false),
    )
    .await
}

/// The widget's GitHub reading: the signed-in user's own PRs and line counts
/// over `range` (defaults to today). A separate, much cheaper fetch than the
/// dashboard's - see `connectors::github::fetch_mine`.
#[tauri::command]
pub async fn pip_github(range: Option<DateRange>) -> Snapshot {
    crate::connectors::github::fetch_mine(range.unwrap_or_default()).await
}

/// The widget's Claude reading: the live 5-hour session and weekly plan meters,
/// off the same throttled `/usage` cache the dashboard uses.
#[tauri::command]
pub async fn pip_claude() -> Snapshot {
    crate::connectors::claude::plan_meters().await
}

/// Shrink the main window into the always-on-top widget, or restore it.
///
/// Window shape is decided here rather than in the frontend so the geometry to
/// restore has one owner, and so the frontend needs no window-mutating
/// permissions beyond dragging.
#[tauri::command]
pub fn set_pip_mode(app: AppHandle, enabled: bool) -> Result<(), String> {
    use tauri::Manager;
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "main window is gone".to_string())?;
    crate::pip::set_mode(&window, enabled).map_err(|e| e.to_string())
}

/// Open a URL in the user's default external browser. Restricted to http(s) so a
/// panel link can never be used to launch an arbitrary scheme locally.
#[tauri::command]
pub fn open_external(url: String) -> Result<(), String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(format!("refusing to open non-http(s) url: {url}"));
    }
    open::that(&url).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `#[serde(flatten)]` is checked by nothing at compile time, and `types.ts`
    /// reads all five of these off the top level - a nested or renamed field
    /// would break the sidebar at runtime with a green build.
    #[test]
    fn a_connector_entry_serializes_flat_and_camel_case() {
        let entry = ConnectorEntry {
            meta: ConnectorMeta {
                id: "claude".into(),
                name: "Claude".into(),
                icon: "claude".into(),
                default_refresh_secs: 60,
            },
            configured: true,
        };

        let json = serde_json::to_value(&entry).unwrap();
        for key in ["id", "name", "icon", "defaultRefreshSecs", "configured"] {
            assert!(json.get(key).is_some(), "missing `{key}` in {json}");
        }
        assert!(json.get("meta").is_none(), "meta stayed nested: {json}");
    }

    /// The sidebar filters, the registry never does. Dropping an unconfigured
    /// connector here instead would make `registry.get(id)` miss the moment a
    /// token is removed, and an in-flight refresh would start failing with
    /// "unknown connector".
    #[test]
    fn every_registered_connector_is_listed() {
        let registry = Registry::with_default_connectors();
        let listed = entries(&registry, &AppConfig::default());
        assert_eq!(listed.len(), registry.all().len());
    }
}
