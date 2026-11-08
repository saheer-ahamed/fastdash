//! The single path every connector fetch takes.
//!
//! Fetch -> write the warm cache -> hand the snapshot back to the caller, so
//! cached data and returned data can never disagree. A UI that broadcasts
//! updates does so from the returned snapshot, never from a second fetch.
//!
//! Nothing here runs on a timer. The UI decides *when* to fetch and fetches
//! only the dashboard that is on screen, only while the app window has focus -
//! a connector nobody is looking at costs nothing. `default_refresh_secs` on the
//! connector's meta is the cadence it asks the UI to poll at while its tab is
//! being watched.

use std::sync::Arc;

use chrono::Utc;

use crate::engine::cache::SnapshotCache;
use crate::engine::connector::{Connector, ConnectorError, FetchCtx, Health, Snapshot};
use crate::engine::range::DateRange;

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

/// Fetch one connector over `range`, store the result in the cache, and return
/// the snapshot.
pub async fn refresh_one(
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
        cache.set(id, snapshot.clone());
    }
    snapshot
}
