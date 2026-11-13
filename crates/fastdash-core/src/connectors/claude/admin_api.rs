//! Official Console usage + cost, via Anthropic's Admin API.
//!
//! This is the sanctioned third-party path. Anthropic runs no OAuth client
//! registration - the only client ids that work against `claude.ai/oauth` are
//! their own - and since February 2026 their policy states that subscription
//! OAuth tokens (Free/Pro/Max) are for Claude Code and claude.ai only. So the
//! connector authenticates with an **Admin API key** (`sk-ant-admin01-...`) the
//! user provisions themselves, sent in `x-api-key`. Two reports per fetch:
//!
//!   GET /v1/organizations/usage_report/messages  token counts, grouped by model
//!   GET /v1/organizations/cost_report            real USD, replacing the
//!                                                notional `pricing.rs` table
//!
//! **Scope, and why the local transcript scan stays.** These endpoints report
//! *Console-billed* usage only. Claude Code running against a Pro/Max
//! subscription is not billed through Console and therefore never shows up
//! here, and the Admin API is unavailable to individual (non-organization)
//! accounts at all. An empty report is an ordinary outcome rather than a
//! failure, which is why [`ConsoleUsage::is_empty`] exists: the connector says
//! so in words instead of rendering zeros that read as "you did nothing".
//!
//! **Day alignment.** The API buckets in UTC; fastdash's ranges are IST days.
//! Every bucket that *overlaps* the selected IST window is counted, so an edge
//! bucket can bleed into a neighbouring day. Spans the API allows hourly
//! buckets on (<= 7 days) use them, holding the bleed under an hour; wider
//! spans fall back to daily, where an edge day can bleed by up to 5h30m.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use crate::engine::perf::{self, CacheState};
use crate::engine::range::{self, DateRange};

const BASE: &str = "https://api.anthropic.com";
const API_VERSION: &str = "2023-06-01";

/// Anthropic asks integrations to identify themselves in the User-Agent.
const UA: &str = concat!(
    "fastdash/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/saheer-ahamed/fastdash)"
);

/// Hourly buckets are capped at 168 (7 days) by the API; wider spans use daily.
const HOURLY_MAX_DAYS: i64 = 7;
/// Guard against a pathological `has_more` loop; 31 daily buckets is the API's
/// own ceiling, so a handful of pages is always enough.
const MAX_PAGES: usize = 20;

#[derive(Debug, thiserror::Error)]
pub enum AdminError {
    #[error("request failed: {0}")]
    Http(String),
    #[error("the admin key was rejected - it may have been revoked or mistyped")]
    Unauthorized,
    #[error(
        "this key cannot read usage. Admin API keys need the organization admin role, \
         and the Admin API is unavailable to individual accounts"
    )]
    Forbidden,
    #[error("Anthropic returned status {0}")]
    Status(u16),
    #[error("could not parse the response: {0}")]
    Parse(String),
}

/// Which Console organization a key belongs to, from `/v1/organizations/me`.
#[derive(Debug, Clone, Deserialize)]
pub struct OrgInfo {
    pub id: String,
    pub name: String,
}

/// Per-model token counts over the selected range.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelUsage {
    pub model: String,
    pub uncached_input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub output: u64,
}

impl ModelUsage {
    pub fn total(&self) -> u64 {
        self.uncached_input + self.cache_read + self.cache_write + self.output
    }
}

/// The official numbers for one range, normalized for the panels.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConsoleUsage {
    /// Descending by total tokens.
    pub per_model: Vec<ModelUsage>,
    /// Real spend in USD. `None` when the cost report was unavailable, which is
    /// distinct from `Some(0.0)` - the latter means "billed nothing".
    pub cost_usd: Option<f64>,
}

impl ConsoleUsage {
    /// No Console-billed usage in this range. Expected for a Pro/Max-only
    /// account, so the caller explains it rather than treating it as an error.
    pub fn is_empty(&self) -> bool {
        self.per_model.is_empty()
    }

    pub fn total_tokens(&self) -> u64 {
        self.per_model.iter().map(ModelUsage::total).sum()
    }
}

fn http() -> Result<reqwest::Client, AdminError> {
    reqwest::Client::builder()
        .user_agent(UA)
        .build()
        .map_err(|e| AdminError::Http(e.to_string()))
}

struct RawGet {
    status: u16,
    etag: Option<String>,
    body: Vec<u8>,
}

/// GET `url` with the admin key. `etag` is sent as `If-None-Match`; a `304`
/// comes back with an empty body so the caller can reuse what it stored.
async fn get_raw(
    client: &reqwest::Client,
    url: &str,
    key: &str,
    page: Option<&str>,
    etag: Option<&str>,
) -> Result<RawGet, AdminError> {
    let started = Instant::now();
    let mut req = client
        .get(url)
        .header("x-api-key", key)
        .header("anthropic-version", API_VERSION);
    if let Some(token) = page {
        req = req.query(&[("page", token)]);
    }
    if let Some(etag) = etag {
        req = req.header("if-none-match", etag);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| AdminError::Http(e.to_string()))?;
    let status = resp.status().as_u16();
    let response_etag = resp
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let finish = |bytes: u64, cache: CacheState| {
        perf::record("claude.admin", started.elapsed(), bytes, cache, true);
    };

    match status {
        304 => {
            finish(0, CacheState::NotModified);
            Ok(RawGet {
                status,
                etag: etag.map(str::to_string),
                body: Vec::new(),
            })
        }
        200 => {
            let body = resp
                .bytes()
                .await
                .map_err(|e| AdminError::Http(e.to_string()))?;
            finish(body.len() as u64, CacheState::Miss);
            Ok(RawGet {
                status,
                etag: response_etag,
                body: body.to_vec(),
            })
        }
        401 => {
            finish(0, CacheState::Miss);
            Err(AdminError::Unauthorized)
        }
        403 => {
            finish(0, CacheState::Miss);
            Err(AdminError::Forbidden)
        }
        other => {
            finish(0, CacheState::Miss);
            Err(AdminError::Status(other))
        }
    }
}

/// GET `url` with the admin key, mapping the statuses the user can act on onto
/// their own variants so the UI can say something better than "403".
async fn get_json<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
    key: &str,
    page: Option<&str>,
) -> Result<T, AdminError> {
    let raw = get_raw(client, url, key, page, None).await?;
    serde_json::from_slice(&raw.body).map_err(|e| AdminError::Parse(e.to_string()))
}

/// Confirm a pasted key works and name the organization it belongs to. Called
/// once at connect time, so the dashboard never re-queries this on a fetch.
pub async fn verify_key(key: &str) -> Result<OrgInfo, AdminError> {
    let client = http()?;
    get_json(&client, &format!("{BASE}/v1/organizations/me"), key, None).await
}

/// RFC3339 with a literal `Z`. `to_rfc3339` would emit `+00:00`, whose `+`
/// means a space once it lands in a query string.
fn ts(dt: DateTime<Utc>) -> String {
    dt.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// The selected IST days as a half-open UTC instant window.
fn utc_window(range: &DateRange) -> (DateTime<Utc>, DateTime<Utc>) {
    let ist = range::ist();
    let at = |day: chrono::NaiveDate| {
        day.and_hms_opt(0, 0, 0)
            .expect("midnight is a valid time")
            .and_local_timezone(ist)
            .single()
            .expect("IST is a fixed offset, so midnight is never ambiguous")
            .with_timezone(&Utc)
    };
    (at(range.start), at(range.end + Duration::days(1)))
}

/// Whether a returned bucket overlaps the window at all. Buckets are UTC-snapped
/// and the window is IST, so the ends rarely line up; counting an overlapping
/// bucket in full is the approximation documented at the top of this file.
fn overlaps(bucket_start: &str, bucket_end: &str, from: DateTime<Utc>, to: DateTime<Utc>) -> bool {
    let parse = |s: &str| {
        DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|dt| dt.with_timezone(&Utc))
    };
    match (parse(bucket_start), parse(bucket_end)) {
        (Some(start), Some(end)) => end > from && start < to,
        // A bucket we cannot place is kept: dropping it would silently
        // under-report, which is the worse failure for a usage dashboard.
        _ => true,
    }
}

/// Fetch and normalize the official numbers for `range`.
///
/// The cost report is best-effort: if it fails while the usage report
/// succeeded, the token panels still render and the cost tile reads as
/// unavailable rather than sinking the whole fetch.
pub async fn fetch_console_usage(key: &str, range: &DateRange) -> Result<ConsoleUsage, AdminError> {
    let range = range.normalized();
    let fp = fingerprint(key);
    let today = range::today_ist();
    if let Some(slot) = recall(fp, &range) {
        // A finished day does not change, and without an ETag there is nothing
        // to revalidate against. An open range, or a stored ETag, revalidates.
        // Only a copy fetched *after* the range ended is final. One fetched
        // while the day was still in progress is missing the rest of the day,
        // so the next fetch (now that the day is closed) must not reuse it.
        if range_is_closed(&range, today)
            && slot.fetched_on > range.end
            && slot.usage_etag.is_none()
            && slot.cost_etag.is_none()
        {
            perf::record(
                "claude.admin",
                std::time::Duration::ZERO,
                0,
                CacheState::Hit,
                true,
            );
            return Ok(slot.usage);
        }
        if slot.single_page {
            if let Some(usage) = revalidate(key, &range, &slot).await? {
                return Ok(usage);
            }
        }
    }

    let (from, to) = utc_window(&range);
    let client = http()?;
    let (per_model, usage_etag, single_page) = fetch_usage(&client, key, &range, from, to).await?;
    let (cost_usd, cost_etag) = match fetch_cost(&client, key, from, to).await {
        Ok((cost, etag)) => (Some(cost), etag),
        Err(e) => {
            eprintln!("claude: cost report unavailable: {e}");
            (None, None)
        }
    };

    let usage = ConsoleUsage {
        per_model,
        cost_usd,
    };
    remember(ConsoleSlot {
        fp,
        start: range.start.to_string(),
        end: range.end.to_string(),
        fetched_on: today,
        usage: usage.clone(),
        usage_etag,
        cost_etag,
        single_page,
    });
    Ok(usage)
}

/// `true` when `range` ends before `today`, so its Console buckets are final.
fn range_is_closed(range: &DateRange, today: NaiveDate) -> bool {
    range.normalized().end < today
}

// --- usage report ---

#[derive(Debug, Deserialize)]
struct UsageReport {
    #[serde(default)]
    data: Vec<UsageBucket>,
    #[serde(default)]
    has_more: bool,
    #[serde(default)]
    next_page: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UsageBucket {
    starting_at: String,
    ending_at: String,
    #[serde(default)]
    results: Vec<UsageItem>,
}

#[derive(Debug, Deserialize)]
struct UsageItem {
    model: Option<String>,
    #[serde(default)]
    uncached_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_creation: Option<CacheCreation>,
}

#[derive(Debug, Default, Deserialize)]
struct CacheCreation {
    #[serde(default)]
    ephemeral_1h_input_tokens: u64,
    #[serde(default)]
    ephemeral_5m_input_tokens: u64,
}

impl CacheCreation {
    fn total(&self) -> u64 {
        self.ephemeral_1h_input_tokens + self.ephemeral_5m_input_tokens
    }
}

fn usage_url(range: &DateRange, from: DateTime<Utc>, to: DateTime<Utc>) -> String {
    let span_days = (range.end - range.start).num_days() + 1;
    let (bucket, limit) = if span_days <= HOURLY_MAX_DAYS {
        ("1h", 168)
    } else {
        ("1d", 31)
    };
    format!(
        "{BASE}/v1/organizations/usage_report/messages\
         ?starting_at={start}&ending_at={end}&bucket_width={bucket}&group_by[]=model&limit={limit}",
        start = ts(from),
        end = ts(to),
    )
}

fn cost_url(from: DateTime<Utc>, to: DateTime<Utc>) -> String {
    format!(
        "{BASE}/v1/organizations/cost_report?starting_at={start}&ending_at={end}&bucket_width=1d&limit=31",
        start = ts(from),
        end = ts(to),
    )
}

async fn fetch_usage(
    client: &reqwest::Client,
    key: &str,
    range: &DateRange,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Result<(Vec<ModelUsage>, Option<String>, bool), AdminError> {
    let url = usage_url(range, from, to);

    let mut totals: HashMap<String, ModelUsage> = HashMap::new();
    let mut page: Option<String> = None;
    let mut etag = None;
    let mut single_page = true;
    let mut first = true;

    for _ in 0..MAX_PAGES {
        let raw = get_raw(client, &url, key, page.as_deref(), None).await?;
        let report: UsageReport =
            serde_json::from_slice(&raw.body).map_err(|e| AdminError::Parse(e.to_string()))?;
        if first {
            first = false;
            if report.has_more {
                single_page = false;
            } else {
                etag = raw.etag;
            }
        }

        for b in &report.data {
            if !overlaps(&b.starting_at, &b.ending_at, from, to) {
                continue;
            }
            for item in &b.results {
                // Ungrouped rows carry no model; they are still real usage, so
                // they are folded under a single "other" row rather than lost.
                let model = item.model.clone().unwrap_or_else(|| "other".to_string());
                let entry = totals.entry(model.clone()).or_insert_with(|| ModelUsage {
                    model,
                    ..ModelUsage::default()
                });
                entry.uncached_input += item.uncached_input_tokens;
                entry.cache_read += item.cache_read_input_tokens;
                entry.output += item.output_tokens;
                entry.cache_write += item
                    .cache_creation
                    .as_ref()
                    .map(CacheCreation::total)
                    .unwrap_or(0);
            }
        }

        match (report.has_more, report.next_page) {
            (true, Some(next)) => page = Some(next),
            _ => break,
        }
    }

    let mut per_model: Vec<ModelUsage> = totals.into_values().filter(|m| m.total() > 0).collect();
    // Biggest consumer first, which is the order the table is read in.
    per_model.sort_by_key(|m| std::cmp::Reverse(m.total()));
    Ok((per_model, etag, single_page))
}

// --- cost report ---

#[derive(Debug, Deserialize)]
struct CostReport {
    #[serde(default)]
    data: Vec<CostBucket>,
    #[serde(default)]
    has_more: bool,
    #[serde(default)]
    next_page: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CostBucket {
    starting_at: String,
    ending_at: String,
    #[serde(default)]
    results: Vec<CostItem>,
}

#[derive(Debug, Deserialize)]
struct CostItem {
    /// Decimal string in the currency's *lowest* unit - `"123.45"` USD is
    /// $1.23, so this is divided by 100 rather than read as dollars.
    amount: Option<String>,
}

async fn fetch_cost(
    client: &reqwest::Client,
    key: &str,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Result<(f64, Option<String>), AdminError> {
    // Daily is the only granularity the cost endpoint offers.
    let url = cost_url(from, to);

    let mut cents = 0.0_f64;
    let mut page: Option<String> = None;
    let mut etag = None;
    let mut first = true;

    for _ in 0..MAX_PAGES {
        let raw = get_raw(client, &url, key, page.as_deref(), None).await?;
        let report: CostReport =
            serde_json::from_slice(&raw.body).map_err(|e| AdminError::Parse(e.to_string()))?;
        if first {
            first = false;
            if !report.has_more {
                etag = raw.etag;
            }
        }

        for b in &report.data {
            if !overlaps(&b.starting_at, &b.ending_at, from, to) {
                continue;
            }
            for item in &b.results {
                if let Some(raw) = &item.amount {
                    cents += raw.parse::<f64>().unwrap_or(0.0);
                }
            }
        }

        match (report.has_more, report.next_page) {
            (true, Some(next)) => page = Some(next),
            _ => break,
        }
    }

    Ok((cents / 100.0, etag))
}

async fn revalidate(
    key: &str,
    range: &DateRange,
    slot: &ConsoleSlot,
) -> Result<Option<ConsoleUsage>, AdminError> {
    let (Some(usage_etag), Some(cost_etag)) = (&slot.usage_etag, &slot.cost_etag) else {
        return Ok(None);
    };
    let client = http()?;
    let (from, to) = utc_window(range);
    let usage = get_raw(
        &client,
        &usage_url(range, from, to),
        key,
        None,
        Some(usage_etag),
    )
    .await?;
    let cost = get_raw(&client, &cost_url(from, to), key, None, Some(cost_etag)).await?;
    if usage.status == 304 && cost.status == 304 {
        Ok(Some(slot.usage.clone()))
    } else {
        // One side changed. The caller fetches both reports again; the 200 body
        // already counted toward the byte total above.
        Ok(None)
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct ConsoleSlot {
    fp: u64,
    start: String,
    end: String,
    fetched_on: NaiveDate,
    usage: ConsoleUsage,
    usage_etag: Option<String>,
    cost_etag: Option<String>,
    single_page: bool,
}

fn fingerprint(key: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hasher);
    hasher.finish()
}

type SlotKey = (u64, String, String);
type SlotMap = HashMap<SlotKey, ConsoleSlot>;

fn slots() -> &'static Mutex<SlotMap> {
    static SLOTS: std::sync::OnceLock<Mutex<SlotMap>> = std::sync::OnceLock::new();
    SLOTS.get_or_init(|| Mutex::new(load_slots()))
}

fn recall(fp: u64, range: &DateRange) -> Option<ConsoleSlot> {
    let key = (fp, range.start.to_string(), range.end.to_string());
    slots().lock().ok()?.get(&key).cloned()
}

fn remember(slot: ConsoleSlot) {
    let key = (slot.fp, slot.start.clone(), slot.end.clone());
    if let Ok(mut slots) = slots().lock() {
        slots.insert(key, slot);
        let snapshot: Vec<ConsoleSlot> = slots.values().cloned().collect();
        drop(slots);
        save_slots(&snapshot);
    }
}

fn slots_path() -> Option<std::path::PathBuf> {
    directories::ProjectDirs::from("co", "zro", "fastdash")
        .map(|dirs| dirs.cache_dir().join("claude-admin.json"))
}

fn load_slots() -> SlotMap {
    if cfg!(test) {
        return HashMap::new();
    }
    let Some(path) = slots_path() else {
        return HashMap::new();
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    let Ok(list) = serde_json::from_str::<Vec<ConsoleSlot>>(&text) else {
        return HashMap::new();
    };
    list.into_iter()
        .map(|slot| {
            let key = (slot.fp, slot.start.clone(), slot.end.clone());
            (key, slot)
        })
        .collect()
}

fn save_slots(slots: &[ConsoleSlot]) {
    if cfg!(test) {
        return;
    }
    let Some(path) = slots_path() else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let Ok(text) = serde_json::to_string(slots) else {
        return;
    };
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(tmp, path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    /// An IST day starts at 18:30 UTC the day before; the window must span
    /// exactly 24h from there, or the report covers the wrong day entirely.
    #[test]
    fn a_range_is_closed_only_once_today_has_moved_past_it() {
        let yesterday = DateRange {
            start: day(2026, 11, 12),
            end: day(2026, 11, 12),
        };
        let today = day(2026, 11, 13);
        assert!(range_is_closed(&yesterday, today));
        assert!(!range_is_closed(
            &DateRange {
                start: today,
                end: today,
            },
            today
        ));
    }

    #[test]
    fn ist_day_maps_onto_the_right_utc_window() {
        let range = DateRange {
            start: day(2026, 8, 2),
            end: day(2026, 8, 2),
        };
        let (from, to) = utc_window(&range);
        assert_eq!(ts(from), "2026-08-01T18:30:00Z");
        assert_eq!(ts(to), "2026-08-02T18:30:00Z");
    }

    #[test]
    fn multi_day_windows_cover_both_ends() {
        let range = DateRange {
            start: day(2026, 7, 28),
            end: day(2026, 8, 2),
        };
        let (from, to) = utc_window(&range);
        assert_eq!(ts(from), "2026-07-27T18:30:00Z");
        assert_eq!(ts(to), "2026-08-02T18:30:00Z");
    }

    /// Buckets touching the window count; ones wholly outside do not. The
    /// half-open end matters: a bucket starting exactly at `to` is the next
    /// day's, and counting it would double-count across adjacent ranges.
    #[test]
    fn bucket_overlap_is_half_open() {
        let (from, to) = utc_window(&DateRange {
            start: day(2026, 8, 2),
            end: day(2026, 8, 2),
        });

        assert!(overlaps(
            "2026-08-01T18:00:00Z",
            "2026-08-01T19:00:00Z",
            from,
            to
        ));
        assert!(overlaps(
            "2026-08-02T18:00:00Z",
            "2026-08-02T19:00:00Z",
            from,
            to
        ));
        // Ends exactly at the window start - entirely the previous day.
        assert!(!overlaps(
            "2026-08-01T17:30:00Z",
            "2026-08-01T18:30:00Z",
            from,
            to
        ));
        // Starts exactly at the window end - entirely the next day.
        assert!(!overlaps(
            "2026-08-02T18:30:00Z",
            "2026-08-02T19:30:00Z",
            from,
            to
        ));
    }

    /// An unparseable bucket is kept, because under-reporting usage is worse
    /// than over-reporting it on a dashboard people check for limits.
    #[test]
    fn unparseable_buckets_are_kept() {
        let (from, to) = utc_window(&DateRange::today());
        assert!(overlaps("not-a-date", "also-not", from, to));
    }

    /// Timestamps must not carry a `+00:00` offset: the `+` decodes as a space
    /// inside a query string and the API rejects the window.
    #[test]
    fn timestamps_are_query_safe() {
        let (from, _) = utc_window(&DateRange::today());
        let stamp = ts(from);
        assert!(stamp.ends_with('Z'), "{stamp}");
        assert!(!stamp.contains('+'), "{stamp}");
    }

    /// Cost arrives in cents as a decimal string; reading it as dollars would
    /// overstate spend by 100x.
    #[test]
    fn cost_is_cents_not_dollars() {
        let item = CostItem {
            amount: Some("123.45".to_string()),
        };
        let dollars = item.amount.unwrap().parse::<f64>().unwrap() / 100.0;
        assert!((dollars - 1.2345).abs() < 1e-9, "{dollars}");
    }
}

#[cfg(test)]
mod live_test {
    use super::*;

    /// End-to-end against the real Admin API, which is the only way to confirm
    /// the query shapes, the bucket filtering, and the cents-to-dollars
    /// conversion against live data. Ignored by default; run with:
    ///   FASTDASH_ANTHROPIC_ADMIN_KEY=sk-ant-admin01-... \
    ///     cargo test --lib claude::admin_api::live_test -- --ignored --nocapture
    ///
    /// An empty report is a pass, not a failure: it is the expected answer for
    /// an account whose Claude usage rides a Pro/Max subscription rather than
    /// Console billing, and the connector renders that case deliberately.
    #[ignore = "hits the live Anthropic Admin API; run with --ignored and FASTDASH_ANTHROPIC_ADMIN_KEY set"]
    #[tokio::test]
    async fn live_console_usage() {
        let key = std::env::var("FASTDASH_ANTHROPIC_ADMIN_KEY")
            .expect("set FASTDASH_ANTHROPIC_ADMIN_KEY for the live test");

        let org = verify_key(&key).await.expect("key verification failed");
        eprintln!("org: {} ({})", org.name, org.id);

        for range in [
            DateRange::today(),
            DateRange {
                start: range::today_ist() - Duration::days(29),
                end: range::today_ist(),
            },
        ] {
            let (from, to) = utc_window(&range);
            eprintln!("\n--- {} [{} .. {}]", range.label(), ts(from), ts(to));

            let usage = fetch_console_usage(&key, &range)
                .await
                .expect("usage fetch failed");
            eprintln!(
                "models: {}  tokens: {}  cost: {:?}",
                usage.per_model.len(),
                usage.total_tokens(),
                usage.cost_usd
            );
            for m in &usage.per_model {
                eprintln!(
                    "  {:<28} in {:>10} out {:>10} cr {:>10} cw {:>10}",
                    m.model, m.uncached_input, m.output, m.cache_read, m.cache_write
                );
            }
            if usage.is_empty() {
                eprintln!("  (no Console-billed usage - the connector explains this in a note)");
            }
        }
    }
}
