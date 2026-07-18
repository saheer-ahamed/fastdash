//! In-memory snapshot cache.
//!
//! The scheduler writes the latest `Snapshot` per connector id here; the UI
//! reads it via the `get_cached` IPC command. "Fast" means the UI always reads
//! this warm cache and never blocks on the network.

use std::collections::HashMap;
use std::sync::RwLock;

use crate::engine::connector::Snapshot;

/// Thread-safe map of `connector id -> latest Snapshot`. Reads clone out so a
/// caller never holds the lock, keeping the critical section tiny.
#[derive(Default)]
pub struct SnapshotCache {
    inner: RwLock<HashMap<String, Snapshot>>,
}

impl SnapshotCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Latest snapshot for `id`, or `None` if nothing has been fetched yet.
    pub fn get(&self, id: &str) -> Option<Snapshot> {
        self.inner.read().unwrap().get(id).cloned()
    }

    /// Store (overwrite) the snapshot for `id`.
    pub fn set(&self, id: impl Into<String>, snapshot: Snapshot) {
        self.inner.write().unwrap().insert(id.into(), snapshot);
    }
}
