//! Process-local caches that let a repeated GitHub fetch cost almost nothing.
//!
//! The dashboard re-fetches on a timer while its tab is watched, and most of
//! those fetches ask GitHub the same questions and get the same answers. Two
//! separate mechanisms turn that into near-zero work:
//!
//!   - **A scope's search results** are cached against a one-request probe of
//!     the same range. The Search API offers no conditional requests - it sends
//!     `Cache-Control: no-cache` and no `ETag` - so the stand-in is asking it
//!     the cheapest possible question first (see `GithubClient::search_probe`)
//!     and re-running the real, paginated queries only when the answer moved.
//!   - **PR enrichment** is cached against the PR's `updated_at`. GraphQL has
//!     no conditional requests and bills points for every node it serves, so
//!     the only way not to pay is not to ask: a PR whose `updated_at` has not
//!     moved cannot have changed its title, state, or line counts.
//!
//! Everything here is keyed by a fingerprint of the token as well, so two
//! accounts on the same dashboard never read each other's entries. The caches
//! live for the life of the process only - they are a way to avoid re-asking,
//! never a store of record, and losing them costs one full fetch.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use super::client::{EnrichedPr, PrRef, SearchProbe, ViewerProfile};
use super::ScopeSets;

/// Search result sets held at once - one per scope per date range, so this
/// covers several orgs across a handful of ranges before the coldest is
/// dropped.
const SEARCH_CAPACITY: usize = 32;
/// Enriched PRs held at once, comfortably above a busy org's day.
const ENRICH_CAPACITY: usize = 4096;

/// A bounded map that evicts whatever was touched longest ago.
///
/// Not a real LRU - it is a `HashMap` plus a monotonic use counter, and
/// eviction is a linear scan for the smallest one. At these capacities that
/// scan happens once per insert past the cap and is far cheaper than the HTTP
/// round trip it exists to avoid.
struct Bounded<K, V> {
    entries: HashMap<K, (u64, V)>,
    clock: u64,
    capacity: usize,
}

impl<K: Eq + Hash + Clone, V> Bounded<K, V> {
    fn new(capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            clock: 0,
            capacity,
        }
    }

    fn get(&mut self, key: &K) -> Option<&V> {
        self.clock += 1;
        let clock = self.clock;
        let (used, value) = self.entries.get_mut(key)?;
        *used = clock;
        Some(value)
    }

    fn insert(&mut self, key: K, value: V) {
        self.clock += 1;
        self.entries.insert(key, (self.clock, value));
        while self.entries.len() > self.capacity {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, (used, _))| *used)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            self.entries.remove(&oldest);
        }
    }
}

/// `(token, scope, range bounds)`. The bounds are part of the key because a
/// probe only speaks for the window it was taken over.
type SearchKey = (u64, String, String);
/// `(token, owner/repo, number, updated_at)`. A PR with no `updated_at` is
/// never cached: there would be nothing to invalidate it.
type EnrichKey = (u64, String, u64, String);

struct Caches {
    search: Bounded<SearchKey, (SearchProbe, ScopeSets)>,
    enrich: Bounded<EnrichKey, EnrichedPr>,
    /// Login and join date behind a token. Neither can change for a given
    /// token, so this is filled once and read for the life of the process.
    viewers: HashMap<u64, ViewerProfile>,
}

fn caches() -> &'static Mutex<Caches> {
    static CACHES: std::sync::OnceLock<Mutex<Caches>> = std::sync::OnceLock::new();
    CACHES.get_or_init(|| {
        Mutex::new(Caches {
            search: Bounded::new(SEARCH_CAPACITY),
            enrich: Bounded::new(ENRICH_CAPACITY),
            viewers: HashMap::new(),
        })
    })
}

/// How much the caches saved, counted since the process started. Only the
/// live diagnostics read this; the counters themselves are always kept, since
/// an atomic increment is nothing next to the request it stands in for.
static SCOPES_REUSED: AtomicU64 = AtomicU64::new(0);
static SCOPES_SEARCHED: AtomicU64 = AtomicU64::new(0);
static ENRICH_HITS: AtomicU64 = AtomicU64::new(0);
static ENRICH_MISSES: AtomicU64 = AtomicU64::new(0);

/// `(scopes reused, scopes re-searched, PRs from cache, PRs asked for)`.
#[cfg(test)]
pub fn savings() -> (u64, u64, u64, u64) {
    (
        SCOPES_REUSED.load(Ordering::Relaxed),
        SCOPES_SEARCHED.load(Ordering::Relaxed),
        ENRICH_HITS.load(Ordering::Relaxed),
        ENRICH_MISSES.load(Ordering::Relaxed),
    )
}

/// A non-reversible fingerprint of a token, used to scope cache keys to the
/// account they were fetched for. The token itself is never stored.
pub fn token_fingerprint(token: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    token.hash(&mut hasher);
    hasher.finish()
}

/// The results last searched for this scope and range, but only if `probe`
/// still matches the one they were stored with - otherwise something in the
/// window moved and they have to be searched again.
pub fn unchanged_scope(
    token: u64,
    scope: &str,
    bounds: &str,
    probe: &SearchProbe,
) -> Option<ScopeSets> {
    let hit = {
        let mut caches = caches().lock().ok()?;
        let (stored, sets) = caches
            .search
            .get(&(token, scope.to_string(), bounds.to_string()))?;
        (stored == probe).then(|| sets.clone())
    };
    match hit {
        Some(sets) => {
            SCOPES_REUSED.fetch_add(1, Ordering::Relaxed);
            Some(sets)
        }
        None => None,
    }
}

pub fn store_scope(token: u64, scope: &str, bounds: &str, probe: SearchProbe, sets: &ScopeSets) {
    SCOPES_SEARCHED.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut caches) = caches().lock() {
        caches.search.insert(
            (token, scope.to_string(), bounds.to_string()),
            (probe, sets.clone()),
        );
    }
}

fn enrich_key(token: u64, pr: &PrRef) -> Option<EnrichKey> {
    let updated = pr.updated_at?;
    Some((
        token,
        format!("{}/{}", pr.owner, pr.repo),
        pr.number,
        updated.to_rfc3339(),
    ))
}

/// Split PRs into the enrichment already held for their current `updated_at`
/// and the refs that still have to be asked for.
pub fn take_enriched(token: u64, prs: &[PrRef]) -> (Vec<EnrichedPr>, Vec<PrRef>) {
    let Ok(mut caches) = caches().lock() else {
        return (Vec::new(), prs.to_vec());
    };
    let mut hits = Vec::new();
    let mut misses = Vec::new();
    for pr in prs {
        match enrich_key(token, pr).and_then(|k| caches.enrich.get(&k).cloned()) {
            Some(hit) => {
                ENRICH_HITS.fetch_add(1, Ordering::Relaxed);
                hits.push(hit)
            }
            None => {
                ENRICH_MISSES.fetch_add(1, Ordering::Relaxed);
                misses.push(pr.clone())
            }
        }
    }
    (hits, misses)
}

/// Remember enrichment against the `updated_at` the PR had when it was asked
/// for. `prs` is the batch that was requested; results are matched back to it
/// by `owner/repo` and number, since GraphQL may answer for only some of them.
pub fn store_enriched(token: u64, prs: &[PrRef], enriched: &[EnrichedPr]) {
    let Ok(mut caches) = caches().lock() else {
        return;
    };
    for pr in prs {
        let Some(key) = enrich_key(token, pr) else {
            continue;
        };
        let Some(found) = enriched
            .iter()
            .find(|e| e.number == pr.number && e.name_with_owner.eq_ignore_ascii_case(&key.1))
        else {
            continue;
        };
        caches.enrich.insert(key, found.clone());
    }
}

pub fn viewer_profile(token: u64) -> Option<ViewerProfile> {
    caches().lock().ok()?.viewers.get(&token).cloned()
}

pub fn store_viewer_profile(token: u64, profile: &ViewerProfile) {
    if let Ok(mut caches) = caches().lock() {
        caches.viewers.insert(token, profile.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Eviction has to drop the least recently *used* entry, not the least
    /// recently inserted one: the page a 60s poll keeps revalidating is the
    /// one whose `ETag` is worth keeping, however old it is.
    #[test]
    fn a_full_cache_evicts_what_nobody_reads() {
        let mut bounded: Bounded<u32, &str> = Bounded::new(2);
        bounded.insert(1, "kept");
        bounded.insert(2, "cold");
        assert_eq!(bounded.get(&1), Some(&"kept"));
        bounded.insert(3, "new");

        assert_eq!(bounded.get(&1), Some(&"kept"), "read recently, so kept");
        assert_eq!(bounded.get(&2), None, "untouched, so evicted");
        assert_eq!(bounded.get(&3), Some(&"new"));
    }

    /// A PR GitHub reports no `updated_at` for has no invalidation signal, so
    /// caching its enrichment would pin stale line counts forever.
    #[test]
    fn a_pr_without_an_update_time_is_never_cached() {
        let pr = PrRef {
            owner: "acme".into(),
            repo: "api".into(),
            number: 7,
            updated_at: None,
        };
        assert!(enrich_key(1, &pr).is_none());
    }
}
