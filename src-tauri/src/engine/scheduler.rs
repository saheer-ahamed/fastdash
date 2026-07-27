//! Background refresh scheduler.
//!
//! For each connector in the `Registry` a task loops on its own
//! `default_refresh_secs` cadence: fetch -> write the cache -> emit a
//! `connector:update` event so the UI updates live. The very first tick fires
//! immediately on startup, warming the cache. Manual refresh reuses the same
//! `refresh_one` path (see `ipc::fetch_connector`), so cached data, emitted
//! events, and returned data can never disagree.

use std::sync::{Arc, RwLock};
use std::time::Duration;

use chrono::Utc;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::engine::cache::SnapshotCache;
use crate::engine::config::AppConfig;
use crate::engine::connector::{Connector, ConnectorError, FetchCtx, Health, Snapshot};
use crate::engine::range::DateRange;
use crate::engine::registry::Registry;

/// Event name the UI subscribes to for live panel updates.
pub const UPDATE_EVENT: &str = "connector:update";

/// Payload of `connector:update`: which connector, which day range it covers,
/// and its fresh snapshot. The range travels with the event so the UI can file
/// a background refresh under the right cache slot instead of overwriting a
/// different range the user is currently looking at.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConnectorUpdate {
    id: String,
    range: DateRange,
    snapshot: Snapshot,
}

/// Turn a fetch error into a snapshot carrying the matching health status, so
/// the UI can render a status dot / banner instead of silently dropping it.
fn error_snapshot(err: &ConnectorError) -> Snapshot {
    let status = match err {
        ConnectorError::Auth(message) => Health::NeedsAuth {
            message: message.clone(),
        },
        ConnectorError::RateLimited => Health::RateLimited {
            retry_after_secs: None,
        },
        ConnectorError::Other(message) => Health::Error {
            message: message.clone(),
        },
    };
    Snapshot {
        status,
        panels: vec![],
        fetched_at: Utc::now(),
        next_refresh_secs: None,
    }
}

/// Fetch one connector, store the result in the cache, emit `connector:update`,
/// and return the snapshot. Shared by the periodic loop and manual refresh.
pub async fn refresh_one(
    app: &AppHandle,
    connector: &Arc<dyn Connector>,
    cache: &SnapshotCache,
    timezone: String,
    range: DateRange,
) -> Snapshot {
    let range = range.normalized();
    let ctx = FetchCtx {
        timezone,
        range: range.clone(),
    };
    let snapshot = match connector.fetch(&ctx).await {
        Ok(snapshot) => snapshot,
        Err(err) => error_snapshot(&err),
    };

    let id = connector.meta().id;
    // The warm-start cache holds one snapshot per connector and `get_cached`
    // has no range to ask for, so only the default (today) view is stored -
    // a historical range the user opened must never become what the app shows
    // on next launch.
    if range == DateRange::today() {
        cache.set(id.clone(), snapshot.clone());
    }
    let _ = app.emit(
        UPDATE_EVENT,
        ConnectorUpdate {
            id,
            range,
            snapshot: snapshot.clone(),
        },
    );
    snapshot
}

/// Spawn one refresh loop per connector. Each loop fetches immediately, then
/// sleeps for the connector's cadence and repeats. The tasks are independent and
/// never block each other or the UI.
pub fn start(
    app: AppHandle,
    registry: Arc<Registry>,
    cache: Arc<SnapshotCache>,
    config: Arc<RwLock<AppConfig>>,
) {
    for connector in registry.all() {
        let connector = connector.clone();
        let app = app.clone();
        let cache = cache.clone();
        let config = config.clone();
        // Never sleep 0s (would busy-loop); clamp to at least 1s.
        let interval = Duration::from_secs(connector.meta().default_refresh_secs.max(1));

        tauri::async_runtime::spawn(async move {
            loop {
                // Read the current timezone fresh each tick so a config change
                // takes effect without a restart. Guard dropped before await.
                let timezone = config.read().unwrap().timezone.clone();
                // The background loop always covers today; other ranges are
                // historical and only fetched on demand from the UI.
                refresh_one(&app, &connector, &cache, timezone, DateRange::today()).await;
                tokio::time::sleep(interval).await;
            }
        });
    }
}
