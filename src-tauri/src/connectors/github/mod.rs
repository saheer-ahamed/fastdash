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
//! each with its own PAT in the OS keychain. Multi-account selection is not yet
//! wired (see `config.rs`); one account is resolved per fetch for now.

mod aggregate;
mod client;
mod config;
pub mod device_flow;

use std::collections::HashMap;

use async_trait::async_trait;
use chrono::{FixedOffset, Utc};

use crate::engine::connector::Health;
use crate::engine::connector::{Connector, ConnectorError, ConnectorMeta, FetchCtx, Snapshot};
use crate::engine::i18n;

use aggregate::{LineContrib, PrEntry, PrState, Rollup};
use client::{EnrichedPr, GithubClient, GithubError, PrRef, SearchItem};
use config::GithubConfig;

const REFRESH_SECS: u64 = 60;

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
        let author = e
            .author
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
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

    let panels = aggregate::build_panels(&rollup, ist);
    Ok(Snapshot::ok(panels, Some(REFRESH_SECS)))
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
}
