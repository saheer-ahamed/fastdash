//! The single path every connector fetch takes.
//!
//! Fetch -> write the warm cache -> hand the snapshot back to the caller, so
//! cached data and returned data can never disagree. A UI that broadcasts
//! updates does so from the returned snapshot, never from a second fetch.
//!
//! Nothing here runs on a timer. The UI fetches the dashboard that is on screen
//! when that view has no snapshot yet (launch, or the first time the tab or
//! range is opened) and when the user hits Refresh. Focus does not gate it, and
//! there is no interval. `default_refresh_secs` on the connector's meta is a
//! leftover hint for callers that still want a cadence; this path does not
//! schedule one.

use std::sync::Arc;

use chrono::Utc;

use crate::engine::cache::SnapshotCache;
use crate::engine::connector::{Connector, ConnectorError, FetchCtx, Health, Snapshot};
use crate::engine::perf;
use crate::engine::range::DateRange;

/// Whether the UI should hit the network for a view.
///
/// A view with no snapshot is fetched the first time it is shown. After that
/// only a manual refresh fetches. Focus and age are not reasons to fetch.
pub fn should_fetch(has_snapshot: bool, manual: bool) -> bool {
    manual || !has_snapshot
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

/// Fetch one connector over `range`, store the result in the cache, and return
/// the snapshot.
pub async fn refresh_one(
    connector: &Arc<dyn Connector>,
    cache: &SnapshotCache,
    timezone: String,
    range: DateRange,
) -> Snapshot {
    let _span = perf::span(fetch_span(connector.meta().id.as_str())).recent();
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

fn fetch_span(id: &str) -> &'static str {
    match id {
        "claude" => "fetch.claude",
        "github" => "fetch.github",
        "sentry" => "fetch.sentry",
        _ => "fetch.other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_view_fetches_only_when_it_has_no_data_or_the_user_asks() {
        assert!(should_fetch(false, false), "first open");
        assert!(should_fetch(true, true), "manual refresh");
        assert!(!should_fetch(true, false), "cached view stays put");
    }
}
