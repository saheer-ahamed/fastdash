//! Opt-in timings for startup, rendering, and connector fetches.
//!
//! Disabled unless `FASTDASH_PERF` is `1` / `true` / `on` / `yes`, or the UI
//! turns developer mode on. A disabled call is a single atomic load: no clock
//! read, no allocation, no log line. When enabled, samples accumulate into
//! totals the UI can draw and, if the env var was what turned it on, each
//! sample is also printed to stderr.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use web_time::{Duration, Instant};

/// How a sample relates to work that was already stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CacheState {
    /// The bytes were fetched.
    Miss,
    /// A local copy answered it; nothing was read from the network or disk.
    Hit,
    /// The server confirmed the stored response (`304 Not Modified`).
    NotModified,
    /// A stored copy was updated from a smaller delta (appended transcript
    /// bytes, a couple of changed pull requests, ...).
    Delta,
}

#[derive(Default)]
struct Totals {
    count: u64,
    total_ns: u128,
    bytes: u64,
    hit: u64,
    miss: u64,
    not_modified: u64,
    delta: u64,
}

struct State {
    totals: BTreeMap<&'static str, Totals>,
    recent: Vec<String>,
}

impl State {
    fn new() -> Self {
        Self {
            totals: BTreeMap::new(),
            recent: Vec::new(),
        }
    }
}

static ENABLED: AtomicBool = AtomicBool::new(false);
/// Stderr logging follows the env var only, so developer mode can show the
/// overlay without flooding a console the GUI build does not have.
static LOG: AtomicBool = AtomicBool::new(false);

fn state() -> &'static Mutex<State> {
    static STATE: std::sync::OnceLock<Mutex<State>> = std::sync::OnceLock::new();
    STATE.get_or_init(|| Mutex::new(State::new()))
}

/// `FASTDASH_PERF=1` (also `true`, `on`, `yes`).
pub fn env_enabled() -> bool {
    matches!(
        std::env::var("FASTDASH_PERF")
            .ok()
            .as_deref()
            .map(str::trim),
        Some("1" | "true" | "on" | "yes")
    )
}

/// Read the env var once at process start. Developer mode can still enable the
/// overlay later via [`sync_enabled`].
pub fn init() {
    if env_enabled() {
        LOG.store(true, Ordering::Relaxed);
        ENABLED.store(true, Ordering::Relaxed);
    }
}

/// Keep collection on while developer mode is on. The env var wins over a
/// later toggle so a measurement session cannot be silenced by the UI.
pub fn sync_enabled(dev_mode: bool) {
    if env_enabled() {
        LOG.store(true, Ordering::Relaxed);
        ENABLED.store(true, Ordering::Relaxed);
        return;
    }
    ENABLED.store(dev_mode, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// A clock that only starts when collection is on.
pub struct Span {
    name: &'static str,
    start: Option<Instant>,
    cache: CacheState,
    bytes: u64,
    recent: bool,
}

impl Span {
    /// Also keep a one-line trace. Render samples skip this; they would crowd
    /// out the fetches the overlay is there to show.
    pub fn recent(mut self) -> Self {
        self.recent = true;
        self
    }

    pub fn set_bytes(&mut self, bytes: u64) {
        self.bytes = bytes;
    }

    pub fn set_cache(&mut self, cache: CacheState) {
        self.cache = cache;
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        let Some(start) = self.start.take() else {
            return;
        };
        record(
            self.name,
            start.elapsed(),
            self.bytes,
            self.cache,
            self.recent,
        );
    }
}

pub fn span(name: &'static str) -> Span {
    if !enabled() {
        return Span {
            name,
            start: None,
            cache: CacheState::Miss,
            bytes: 0,
            recent: false,
        };
    }
    Span {
        name,
        start: Some(Instant::now()),
        cache: CacheState::Miss,
        bytes: 0,
        recent: false,
    }
}

pub fn record(name: &'static str, elapsed: Duration, bytes: u64, cache: CacheState, recent: bool) {
    if !enabled() {
        return;
    }
    let ns = elapsed.as_nanos();
    if LOG.load(Ordering::Relaxed) {
        eprintln!(
            "perf {name} {ms}ms bytes={bytes} {cache:?}",
            ms = elapsed.as_millis()
        );
    }
    let Ok(mut state) = state().lock() else {
        return;
    };
    let totals = state.totals.entry(name).or_default();
    totals.count += 1;
    totals.total_ns += ns;
    totals.bytes += bytes;
    match cache {
        CacheState::Hit => totals.hit += 1,
        CacheState::Miss => totals.miss += 1,
        CacheState::NotModified => totals.not_modified += 1,
        CacheState::Delta => totals.delta += 1,
    }
    if recent {
        let line = format!(
            "{name}  {}  {}  {}",
            fmt_ms_one(ns),
            fmt_bytes(bytes),
            cache_word(cache)
        );
        state.recent.insert(0, line);
        state.recent.truncate(6);
    }
}

/// The overlay text, or `None` when collection is off (callers should not even
/// build the widget in that case).
pub fn hud() -> Option<String> {
    if !enabled() {
        return None;
    }
    let Ok(state) = state().lock() else {
        return None;
    };
    let mut out = String::from("perf");
    if state.totals.is_empty() {
        out.push_str("\n(no samples yet)");
        return Some(out);
    }
    for (name, totals) in &state.totals {
        out.push('\n');
        out.push_str(name);
        out.push_str("  ");
        out.push_str(&format!(
            "n={} {} {}",
            totals.count,
            fmt_ms(totals.total_ns, totals.count),
            fmt_bytes(totals.bytes)
        ));
        let kinds = fmt_kinds(totals);
        if !kinds.is_empty() {
            out.push(' ');
            out.push_str(&kinds);
        }
    }
    if !state.recent.is_empty() {
        out.push_str("\n--");
        for line in &state.recent {
            out.push('\n');
            out.push_str(line);
        }
    }
    Some(out)
}

fn cache_word(cache: CacheState) -> &'static str {
    match cache {
        CacheState::Hit => "hit",
        CacheState::Miss => "miss",
        CacheState::NotModified => "304",
        CacheState::Delta => "delta",
    }
}

fn fmt_kinds(totals: &Totals) -> String {
    let mut parts = Vec::new();
    if totals.hit > 0 {
        parts.push(format!("hit {}", totals.hit));
    }
    if totals.miss > 0 {
        parts.push(format!("miss {}", totals.miss));
    }
    if totals.not_modified > 0 {
        parts.push(format!("304 {}", totals.not_modified));
    }
    if totals.delta > 0 {
        parts.push(format!("delta {}", totals.delta));
    }
    parts.join(" ")
}

fn fmt_ms(total_ns: u128, count: u64) -> String {
    if count == 0 {
        return "-".to_string();
    }
    fmt_ms_one(total_ns / u128::from(count))
}

fn fmt_ms_one(ns: u128) -> String {
    let ms = ns as f64 / 1_000_000.0;
    if ms >= 100.0 {
        format!("{ms:.0}ms")
    } else {
        format!("{ms:.1}ms")
    }
}

fn fmt_bytes(n: u64) -> String {
    if n >= 1024 * 1024 {
        format!("{:.1}MiB", n as f64 / 1024.0 / 1024.0)
    } else if n >= 1024 {
        format!("{:.1}KiB", n as f64 / 1024.0)
    } else {
        format!("{n}B")
    }
}

#[cfg(test)]
pub fn reset_for_test() {
    ENABLED.store(false, Ordering::Relaxed);
    LOG.store(false, Ordering::Relaxed);
    if let Ok(mut state) = state().lock() {
        state.totals.clear();
        state.recent.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The collector is process-global, so the two tests must not interleave.
    fn lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn disabled_recording_allocates_nothing_visible() {
        let _guard = lock();
        reset_for_test();
        record(
            "test.silent",
            Duration::from_millis(5),
            99,
            CacheState::Miss,
            true,
        );
        assert!(hud().is_none(), "a disabled collector has no overlay");
        ENABLED.store(true, Ordering::Relaxed);
        assert!(
            hud().is_some_and(|text| !text.contains("test.silent")),
            "samples taken while disabled must not appear later"
        );
        reset_for_test();
    }

    #[test]
    fn enabled_recording_rolls_up_cache_state() {
        let _guard = lock();
        reset_for_test();
        ENABLED.store(true, Ordering::Relaxed);
        record(
            "test.fetch",
            Duration::from_millis(10),
            1000,
            CacheState::Miss,
            true,
        );
        record(
            "test.fetch",
            Duration::from_millis(0),
            0,
            CacheState::NotModified,
            true,
        );
        let text = hud().expect("overlay");
        assert!(text.contains("test.fetch"));
        assert!(text.contains("n=2"));
        assert!(text.contains("miss 1"));
        assert!(text.contains("304 1"));
        assert!(text.contains("--"));
        reset_for_test();
    }
}
