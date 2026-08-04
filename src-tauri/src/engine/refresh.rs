//! The single path every connector fetch takes.
//!
//! Fetch -> write the warm cache -> emit a `connector:update` event -> hand the
//! snapshot back to the caller, so cached data, emitted events, and returned
//! data can never disagree.
//!
//! Nothing here runs on a timer. The frontend decides *when* to fetch (via
//! `ipc::fetch_connector`) and fetches only the dashboard that is on screen,
//! only while the app window has focus - a connector nobody is looking at costs
//! nothing. `default_refresh_secs` on the connector's meta is the cadence it
//! asks the UI to poll at while its tab is being watched.

use std::sync::Arc;

use chrono::Utc;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::engine::cache::SnapshotCache;
use crate::engine::connector::{Connector, ConnectorError, FetchCtx, Health, Snapshot};
use crate::engine::range::DateRange;

/// Event name the UI subscribes to for live panel updates.
pub const UPDATE_EVENT: &str = "connector:update";

/// Payload of `connector:update`: which connector, which day range it covers,
/// and its fresh snapshot. The range travels with the event so the UI files the
/// result under the right cache slot instead of overwriting a different range
/// the user is currently looking at.
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
        ConnectorError::Misconfigured(message) => Health::Misconfigured {
            message: message.clone(),
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
/// and return the snapshot.
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
    // The warm cache holds one snapshot per connector and `get_cached` has no
    // range to ask for, so only the default (today) view is stored - a
    // historical range the user opened must never become what a later reader
    // of the cache takes for "now".
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
