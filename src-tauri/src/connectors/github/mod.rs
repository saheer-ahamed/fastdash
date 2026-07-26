//! GitHub connector.
//!
//! Per selected org, uses the REST Search API for the date-filtered PR sets
//! (opened / merged / closed-without-merge / still-open for the IST day), then
//! a single batched GraphQL enrichment for additions/deletions/state on the
//! MERGED-today set. Emits a `StatCards` header plus three tables: PR counts
//! per contributor, line contributions per contributor (based on PRs MERGED
//! today), and the PR list with repos.
//!
//! Supports multiple accounts (work `saheer-zro`, personal `saheer-ahamed`),
//! each with its own PAT in the OS keychain. The scheduler's `fetch` renders the
//! first account (all orgs) for the sidebar status dot; the UI drives per-account
//! and per-org sub-tabs through `fetch_account` (see `ipc::github_fetch`).

mod aggregate;
mod client;
mod config;
pub mod device_flow;

use std::collections::HashMap;

use async_trait::async_trait;
use chrono::{DateTime, Datelike, Duration, FixedOffset, NaiveDate, Utc};

use crate::engine::connector::Health;
use crate::engine::connector::{Connector, ConnectorError, ConnectorMeta, FetchCtx, Snapshot};
use crate::engine::i18n;

use aggregate::{LineContrib, PrEntry, PrState, Rollup};
use client::{CalendarWindow, EnrichedPr, GithubClient, GithubError, PrRef, SearchItem};
use config::GithubConfig;

const REFRESH_SECS: u64 = 60;
/// Year tabs on the contribution heatmap (current year plus the previous four),
/// clamped to the years the account has actually existed.
const HEATMAP_YEARS: i32 = 5;

pub struct GithubConnector;

impl GithubConnector {
    pub fn new() -> Self {
        GithubConnector
    }
}

#[async_trait]
impl Connector for GithubConnector {
    fn meta(&self) -> ConnectorMeta {
        ConnectorMeta {
            id: "github".into(),
            name: "GitHub".into(),
            icon: "github".into(),
            default_refresh_secs: REFRESH_SECS,
        }
    }

    async fn fetch(&self, _ctx: &FetchCtx) -> Result<Snapshot, ConnectorError> {
        // "Today" is fixed to the IST day per the design (PRs near midnight are
        // attributed by IST datetime bounds). `_ctx.timezone` is ignored for now.
        let Some(cfg) = GithubConfig::resolve() else {
            return Ok(Snapshot::needs_auth(i18n::t("github.needsAuth")));
        };

        match run_fetch(&cfg).await {
            Ok(snapshot) => Ok(snapshot),
            Err(GithubError::RateLimited { retry_after_secs }) => {
                Ok(rate_limited_snapshot(retry_after_secs))
            }
            Err(e) => Err(ConnectorError::Other(e.to_string())),
        }
    }
}

/// Fetch a specific account's dashboard, optionally scoped to a single org
/// (an org-filter sub-tab; `None` means all of the account's orgs). Always
/// returns a `Snapshot` carrying the right `Health` (needsAuth / rateLimited /
/// error) so the UI can render a banner instead of surfacing a raw error.
pub async fn fetch_account(label: String, org: Option<String>) -> Snapshot {
    let Some(cfg) = GithubConfig::for_account(&label, org.as_deref()) else {
        return Snapshot::needs_auth(i18n::t("github.needsAuth"));
    };
    match run_fetch(&cfg).await {
        Ok(snapshot) => snapshot,
        Err(GithubError::RateLimited { retry_after_secs }) => {
            rate_limited_snapshot(retry_after_secs)
        }
        Err(e) => Snapshot {
            status: Health::Error {
                message: e.to_string(),
            },
            panels: vec![],
            fetched_at: Utc::now(),
            next_refresh_secs: None,
        },
    }
}

/// The IST fixed offset (UTC+05:30); `east_opt` only fails on out-of-range.
fn ist_offset() -> FixedOffset {
    FixedOffset::east_opt(5 * 3600 + 30 * 60).expect("IST offset is in range")
}

/// RFC3339 bounds for the current IST day, e.g.
/// `2026-07-18T00:00:00+05:30..2026-07-18T23:59:59+05:30`.
fn ist_day_bounds(ist: FixedOffset) -> String {
    let today = Utc::now().with_timezone(&ist).date_naive();
    format!(
        "{day}T00:00:00+05:30..{day}T23:59:59+05:30",
        day = today.format("%Y-%m-%d")
    )
}

async fn run_fetch(cfg: &GithubConfig) -> Result<Snapshot, GithubError> {
    let ist = ist_offset();
    let bounds = ist_day_bounds(ist);
    let client = GithubClient::new(&cfg.token)?;

    let mut rollup = Rollup::default();
    // PRs deduped across all four sets, with per-set outcome flags.
    let mut seen: HashMap<(String, String, u64), SeenPr> = HashMap::new();

    for org in &cfg.orgs {
        let opened = client
            .search_issues(&format!("org:{org} type:pr created:{bounds}"))
            .await?;
        let merged = client
            .search_issues(&format!("org:{org} type:pr merged:{bounds}"))
            .await?;
        let closed = client
            .search_issues(&format!("org:{org} type:pr closed:{bounds} is:unmerged"))
            .await?;
        let still_open = client
            .search_issues(&format!("org:{org} type:pr created:{bounds} is:open"))
            .await?;

        // Independent per-contributor counts (a PR may fall in several buckets).
        count_authors(&opened, &mut rollup.opened);
        count_authors(&merged, &mut rollup.merged);
        count_authors(&closed, &mut rollup.closed);
        count_authors(&still_open, &mut rollup.open);

        // Fold every set into the deduped union with outcome flags.
        for it in opened {
            upsert(&mut seen, it, false, false, false);
        }
        for it in merged {
            upsert(&mut seen, it, true, false, false);
        }
        for it in closed {
            upsert(&mut seen, it, false, true, false);
        }
        for it in still_open {
            upsert(&mut seen, it, false, false, true);
        }
    }

    // Enrich only the MERGED-today set (line contributions are merged-based).
    let merged_refs: Vec<PrRef> = seen
        .values()
        .filter(|s| s.merged)
        .map(|s| s.item.pr_ref())
        .collect();

    let enriched = if merged_refs.is_empty() {
        Vec::new()
    } else {
        client.enrich_prs(&merged_refs).await?
    };

    // Index enrichment by (nameWithOwner, number) for overlay onto the union.
    let mut enrich_by_key: HashMap<(String, u64), &EnrichedPr> = HashMap::new();
    for e in &enriched {
        enrich_by_key.insert((e.name_with_owner.clone(), e.number), e);
    }

    // Line contributions: merged-today PRs attributed to their author.
    for e in &enriched {
        let author = e.author.clone().unwrap_or_else(|| "unknown".to_string());
        if is_bot(&author) {
            continue;
        }
        rollup.line_contribs.push(LineContrib {
            author,
            additions: e.additions,
            deletions: e.deletions,
        });
    }

    // Build the "PRs today" union list.
    for s in seen.values() {
        let author = s.item.author.clone();
        if author.as_deref().map(is_bot).unwrap_or(false) {
            continue;
        }

        let name_with_owner = format!("{}/{}", s.item.owner, s.item.repo);
        let enriched = enrich_by_key.get(&(name_with_owner.clone(), s.item.number));

        let is_merged = s.merged || s.item.merged_at.is_some();
        let is_closed = s.closed_unmerged || s.item.closed_at.is_some();
        let (state, at) = if is_merged {
            let merged_at = enriched
                .and_then(|e| e.merged_at)
                .or(s.item.merged_at)
                .or(s.item.closed_at)
                .or(s.item.created_at);
            (PrState::Merged, merged_at)
        } else if is_closed {
            (PrState::Closed, s.item.closed_at.or(s.item.created_at))
        } else {
            (PrState::Open, s.item.created_at)
        };

        let (additions, deletions) = match (is_merged, enriched) {
            (true, Some(e)) => (Some(e.additions), Some(e.deletions)),
            _ => (None, None),
        };

        rollup.pr_list.push(PrEntry {
            name_with_owner,
            title: s.item.title.clone(),
            url: s.item.html_url.clone(),
            author,
            state,
            additions,
            deletions,
            at,
        });
    }

    // The contribution calendar is viewer-scoped, not org-scoped, so it is the
    // same on every org sub-tab. Best-effort: a GraphQL failure drops the
    // heatmap, never the dashboard.
    let contributions = contributions_panels(&client).await;

    let panels = aggregate::build_panels(&rollup, ist, contributions);
    Ok(Snapshot::ok(panels, Some(REFRESH_SECS)))
}

/// Fetch the viewer's contribution calendars and build the heatmap panel,
/// preceded by a note when the token can only see part of the calendar.
/// Returns an empty vec (and logs) if GitHub won't give them to this token.
async fn contributions_panels(client: &GithubClient) -> Vec<crate::engine::panel::Panel> {
    let profile = match client.viewer_profile().await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("github: contribution heatmap unavailable: {e}");
            return Vec::new();
        }
    };

    let now = Utc::now();
    let windows = calendar_windows(profile.created_at, now);
    let calendars = match client.contribution_calendars(&windows).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("github: contribution calendar fetch failed: {e}");
            return Vec::new();
        }
    };

    let mut panels = Vec::new();
    // Without `read:user` GitHub quietly drops private activity from the
    // calendar and returns a near-empty year rather than an error, which reads
    // as "you did nothing". Only a classic token advertises its scopes, so the
    // warning is shown only when we positively know one is missing.
    if let Some(scopes) = incomplete_scopes(client) {
        panels.push(aggregate::contributions_partial_note(
            &profile.login,
            &scopes,
        ));
    }
    panels.extend(aggregate::contributions_heatmap(
        &profile.login,
        &calendars,
        now.year(),
    ));
    panels
}

/// The token's advertised scopes, but only when `read:user` - the scope the
/// contribution calendar needs to include private activity - is missing.
fn incomplete_scopes(client: &GithubClient) -> Option<String> {
    let scopes = client.seen_scopes()?;
    if scopes.split(',').any(|s| s.trim() == "read:user") {
        None
    } else {
        Some(scopes)
    }
}

/// One window per year tab, most recent first. The current year is the rolling
/// last 12 months (what github.com shows for it); earlier years are Jan 1 to
/// Dec 31. Years before the account existed are dropped.
fn calendar_windows(created_at: Option<DateTime<Utc>>, now: DateTime<Utc>) -> Vec<CalendarWindow> {
    let this_year = now.year();
    let first_year = created_at
        .map(|c| c.year())
        .unwrap_or(this_year - HEATMAP_YEARS + 1)
        .max(this_year - HEATMAP_YEARS + 1);

    (first_year..=this_year)
        .rev()
        .filter_map(|year| {
            // The current year asks for GitHub's default window, which is
            // exactly what the profile page renders.
            let (from, to) = if year == this_year {
                (None, None)
            } else {
                (
                    Some(year_start(year)?.to_rfc3339()),
                    Some((year_start(year + 1)? - Duration::seconds(1)).to_rfc3339()),
                )
            };
            Some(CalendarWindow {
                label: year.to_string(),
                from,
                to,
            })
        })
        .collect()
}

fn year_start(year: i32) -> Option<DateTime<Utc>> {
    NaiveDate::from_ymd_opt(year, 1, 1)?
        .and_hms_opt(0, 0, 0)?
        .and_local_timezone(Utc)
        .single()
}

/// A PR seen across one or more search sets, with its outcome flags.
struct SeenPr {
    item: SearchItem,
    merged: bool,
    closed_unmerged: bool,
    open: bool,
}

fn upsert(
    seen: &mut HashMap<(String, String, u64), SeenPr>,
    item: SearchItem,
    merged: bool,
    closed_unmerged: bool,
    open: bool,
) {
    let entry = seen.entry(item.key()).or_insert_with(|| SeenPr {
        item: item.clone(),
        merged: false,
        closed_unmerged: false,
        open: false,
    });
    entry.merged |= merged;
    entry.closed_unmerged |= closed_unmerged;
    entry.open |= open;
}

/// Tally PR authors into `counts`, skipping bots and missing authors.
fn count_authors(items: &[SearchItem], counts: &mut HashMap<String, u64>) {
    for it in items {
        if let Some(login) = &it.author {
            if is_bot(login) {
                continue;
            }
            *counts.entry(login.clone()).or_insert(0) += 1;
        }
    }
}

/// Filter obvious bot authors (dependabot and any `...[bot]` account).
fn is_bot(login: &str) -> bool {
    let l = login.to_ascii_lowercase();
    l.ends_with("[bot]") || l == "dependabot" || l.starts_with("dependabot")
}

fn rate_limited_snapshot(retry_after_secs: Option<u64>) -> Snapshot {
    Snapshot {
        status: Health::RateLimited { retry_after_secs },
        panels: vec![],
        fetched_at: Utc::now(),
        next_refresh_secs: retry_after_secs.or(Some(REFRESH_SECS)),
    }
}

#[cfg(test)]
mod live_test {
    use super::*;

    /// Live smoke test against the real GitHub API. Ignored by default; run with:
    ///   GITHUB_TOKEN=<token> cargo test -p fastdash github::live_test -- --ignored --nocapture
    /// (get a work-account token via `gh auth token -u saheer-zro`).
    #[ignore = "hits the live GitHub API; run with --ignored and GITHUB_TOKEN set"]
    #[tokio::test]
    async fn live_fetch_smoke() {
        let cfg = GithubConfig::resolve().expect("set GITHUB_TOKEN for the live test");
        eprintln!("orgs: {:?}", cfg.orgs);
        eprintln!("bounds: {}", ist_day_bounds(ist_offset()));

        let snapshot = run_fetch(&cfg).await.expect("fetch failed");
        eprintln!("status: {:?}", snapshot.status);
        eprintln!("panels: {}", snapshot.panels.len());
        let json = serde_json::to_string_pretty(&snapshot.panels).unwrap();
        eprintln!("{json}");
    }

    /// Why a contribution heatmap is empty for a given account: prints the token's
    /// OAuth scopes, the calendar totals GitHub reports for the viewer, and the
    /// PR counts the Search API can see for the same login. Run with:
    ///   cargo test --lib github::live_test::contributions_diag -- --ignored --nocapture
    /// Set FASTDASH_GITHUB_LABEL to pick an account (defaults to the first one).
    #[ignore = "hits the live GitHub API; run with --ignored"]
    #[tokio::test]
    async fn contributions_diag() {
        let label = std::env::var("FASTDASH_GITHUB_LABEL").unwrap_or_default();
        let cfg = if label.is_empty() {
            GithubConfig::resolve()
        } else {
            GithubConfig::for_account(&label, None)
        }
        .expect("no token for that account");
        eprintln!("account: {} orgs: {:?}", cfg.label, cfg.orgs);

        let client = GithubClient::new(&cfg.token).expect("client");
        let profile = client.viewer_profile().await.expect("viewer");
        eprintln!("viewer: {} created {:?}", profile.login, profile.created_at);
        eprintln!("token scopes: {:?}", client.seen_scopes());
        eprintln!("scopes missing read:user: {:?}", incomplete_scopes(&client));

        let breakdown = client
            .contribution_breakdown()
            .await
            .expect("contribution breakdown");
        eprintln!("calendar breakdown: {breakdown:#?}");

        let authored = client
            .search_issues(&format!("author:{} type:pr", profile.login))
            .await
            .map(|v| v.len())
            .unwrap_or(0);
        eprintln!(
            "PRs authored by {} (search, all time): {authored}",
            profile.login
        );
    }
}
