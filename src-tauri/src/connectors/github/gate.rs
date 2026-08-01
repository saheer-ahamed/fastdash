//! Coordination for GitHub dashboard fetches.
//!
//! One dashboard fetch is expensive: four Search API calls per org (each up to
//! ten pages) plus two or three GraphQL calls - against a Search budget of 30
//! requests per minute. Nothing used to stop several of those running at once:
//! the view's own 60s interval, every account/org sub-tab click, the manual
//! Refresh button and the background scheduler all called straight into
//! `run_fetch`. That caused both of the symptoms this module exists to kill:
//!
//! * **Rate limiting.** Flipping through sub-tabs fired a full fetch per click,
//!   with the earlier ones still running, and the Search budget was gone in
//!   seconds.
//! * **Stale data after a refresh.** Overlapping fetches resolve in whatever
//!   order the network decides. A slow fetch started minutes ago could land
//!   *after* a fresh one and repaint the older numbers - which then sat there
//!   until the app was relaunched and the state was thrown away.
//!
//! The gate enforces two rules:
//!
//! * At most one fetch is in flight. Starting a new one cancels the previous,
//!   which stops at its next checkpoint instead of spending more budget. The
//!   background scheduler is the polite exception: it skips its tick rather than
//!   preempting a fetch the user is waiting on (see [`Gate::begin_if_idle`]).
//! * A result younger than [`TTL`] is reused instead of refetched, so the
//!   scheduler tick, the view's interval and a sub-tab click that land close
//!   together cost one round of API calls rather than three.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::engine::connector::Snapshot;

/// How long a fetched snapshot is reused before the API is hit again. Well under
/// the 60s refresh cadence, so it only ever collapses a burst - it never lets a
/// view go stale on its own. The manual Refresh button bypasses it entirely.
const TTL: Duration = Duration::from_secs(20);

/// Cancellation flag handed to an in-flight fetch. A cancelled fetch bails at
/// its next checkpoint instead of spending more of the rate-limit budget.
#[derive(Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    /// A token that is never cancelled, for callers outside the gate (tests).
    #[cfg(test)]
    pub fn none() -> Self {
        Cancel::default()
    }

    pub fn cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    fn is(&self, other: &Cancel) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// The right to be the one fetch in flight. Releasing the gate is tied to the
/// lease's lifetime, so an early return or a panic can never strand it.
pub struct Lease<'a> {
    gate: &'a Gate,
    key: String,
    cancel: Cancel,
}

impl Lease<'_> {
    /// The token to pass down into the fetch so it can bail when superseded.
    pub fn cancel(&self) -> &Cancel {
        &self.cancel
    }
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        self.gate.release(&self.cancel);
    }
}

#[derive(Default)]
struct State {
    /// Cancellation token of the fetch that currently holds the gate.
    inflight: Option<Cancel>,
    /// Most recent snapshot per view key, and when it was produced.
    last: HashMap<String, (Instant, Snapshot)>,
}

#[derive(Default)]
pub struct Gate {
    state: Mutex<State>,
}

/// The process-wide gate. A single instance is what makes "only one GitHub fetch
/// at a time" true across the UI commands and the scheduler alike.
pub fn gate() -> &'static Gate {
    static GATE: OnceLock<Gate> = OnceLock::new();
    GATE.get_or_init(Gate::default)
}

impl Gate {
    /// The cached snapshot for `key`, if it is younger than [`TTL`].
    pub fn fresh(&self, key: &str) -> Option<Snapshot> {
        let state = self.state.lock().ok()?;
        let (at, snapshot) = state.last.get(key)?;
        (at.elapsed() < TTL).then(|| snapshot.clone())
    }

    /// The last snapshot recorded for `key`, however old. Used when a fetch is
    /// cancelled and the caller still owes someone a snapshot.
    pub fn last(&self, key: &str) -> Option<Snapshot> {
        let state = self.state.lock().ok()?;
        state.last.get(key).map(|(_, snapshot)| snapshot.clone())
    }

    /// Take the gate, cancelling whatever fetch held it. Use for anything the
    /// user is waiting on: the newest request always wins.
    pub fn begin(&self, key: &str) -> Lease<'_> {
        let cancel = Cancel::default();
        if let Ok(mut state) = self.state.lock() {
            if let Some(previous) = state.inflight.replace(cancel.clone()) {
                previous.cancel();
            }
        }
        Lease {
            gate: self,
            key: key.to_string(),
            cancel,
        }
    }

    /// Take the gate only if it is free. Returns `None` when a fetch is already
    /// running, so background work yields instead of cancelling a user-visible
    /// fetch (and, either way, spends no extra rate-limit budget).
    pub fn begin_if_idle(&self, key: &str) -> Option<Lease<'_>> {
        let cancel = Cancel::default();
        {
            let mut state = self.state.lock().ok()?;
            if state.inflight.is_some() {
                return None;
            }
            state.inflight = Some(cancel.clone());
        }
        Some(Lease {
            gate: self,
            key: key.to_string(),
            cancel,
        })
    }

    /// Record the result of a lease's fetch. A superseded lease is ignored: its
    /// data is older than whatever cancelled it, and letting it through is
    /// exactly the out-of-order repaint this module exists to prevent.
    pub fn finish(&self, lease: &Lease, snapshot: &Snapshot) {
        if lease.cancel.cancelled() {
            return;
        }
        if let Ok(mut state) = self.state.lock() {
            state
                .last
                .insert(lease.key.clone(), (Instant::now(), snapshot.clone()));
        }
    }

    /// Clear the in-flight slot if `cancel` is still the one holding it. A lease
    /// that was superseded long ago must not release its successor's claim.
    fn release(&self, cancel: &Cancel) {
        if let Ok(mut state) = self.state.lock() {
            if state.inflight.as_ref().is_some_and(|c| c.is(cancel)) {
                state.inflight = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> Snapshot {
        Snapshot::ok(vec![], None)
    }

    #[test]
    fn newest_request_cancels_the_previous_one() {
        let gate = Gate::default();
        let first = gate.begin("a");
        let second = gate.begin("b");

        assert!(
            first.cancel().cancelled(),
            "the older fetch must stand down"
        );
        assert!(!second.cancel().cancelled());
    }

    #[test]
    fn a_superseded_fetch_cannot_publish_its_result() {
        let gate = Gate::default();
        let stale = gate.begin("a");
        let _fresh = gate.begin("a");

        gate.finish(&stale, &snapshot());
        assert!(
            gate.last("a").is_none(),
            "an out-of-order result must never be recorded"
        );
    }

    #[test]
    fn background_work_yields_to_an_in_flight_fetch() {
        let gate = Gate::default();
        let held = gate.begin("a");
        assert!(gate.begin_if_idle("a").is_none());

        drop(held);
        assert!(gate.begin_if_idle("a").is_some(), "the gate must free up");
    }

    #[test]
    fn a_recorded_result_is_reused_within_the_ttl() {
        let gate = Gate::default();
        let lease = gate.begin("a");
        gate.finish(&lease, &snapshot());

        assert!(gate.fresh("a").is_some());
        assert!(gate.fresh("b").is_none());
    }
}
