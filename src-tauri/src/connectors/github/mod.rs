//! GitHub connector.
//!
//! Per selected org, uses the REST Search API for the date-filtered PR sets
//! (created / merged / closed-without-merge over the selected IST day range),
//! then a single batched GraphQL enrichment for additions/deletions/state on
//! the PRs the dashboard reports as merged. Emits a `StatCards` header plus
//! three tables: PR counts per contributor, line contributions per contributor
//! (based on PRs MERGED in the range), and the PR list with repos.
//!
//! The per-contributor columns answer three independent questions about the
//! range plus one about right now, and [`count_activity`] is where that is
//! decided - never at fetch time, so a PR matched by two configured scopes is
//! still counted once:
//!
//! * **Created / Merged / Closed unmerged** are events inside the range, each
//!   asked on its own. A pull request opened last week and merged today is
//!   Merged here and Created in the range covering last week, so the columns are
//!   deliberately *not* a partition of Created and a row is not expected to sum.
//!   Tying Merged to the created cohort instead would answer "what did we open
//!   that has landed", which is not what a dashboard filtered to a day is asked.
//! * **Still open** is the one state reading, because staying open is the
//!   absence of an event: it is the pull requests opened in the range that have
//!   not closed since. **Drafts** narrows it further and never stands beside it.
//!
//! The range comes from the UI's date filter. Created and Merged are applied the
//! way GitHub itself filters, as `created:` / `merged:` qualifiers, so each set
//! matches what the same query shows on github.com; the closed set cannot be
//! (see [`search_scope`]). It defaults to today. The contribution heatmap is
//! deliberately exempt: it is GitHub's own rolling calendar, not a range report.
//!
//! Search hands back at most 1000 results per query and gives no sign that it
//! dropped the rest, so every query's `total_count` is checked and a range
//! wider than that says so in a note above the tables rather than presenting
//! its newest slice as the whole picture.
//!
//! Supports multiple accounts (work `saheer-zro`, personal `saheer-ahamed`),
//! each with its own PAT in the OS keychain. The UI drives per-account and
//! per-org sub-tabs through `fetch_account` (see `ipc::github_fetch`); the
//! `Connector::fetch` below is the trait's generic entry point and renders the
//! first account, all orgs.

mod aggregate;
mod cache;
mod client;
mod config;
pub mod device_flow;
mod gate;

use std::collections::HashMap;

use async_trait::async_trait;
use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};

use crate::engine::config::AppConfig;
use crate::engine::connector::Health;
use crate::engine::connector::{Connector, ConnectorError, ConnectorMeta, FetchCtx, Snapshot};
use crate::engine::i18n;
use crate::engine::range::{self, DateRange};

use aggregate::{LineContrib, PrEntry, PrState, Rollup};
use client::{CalendarWindow, EnrichedPr, GithubClient, GithubError, PrRef, SearchItem};
use config::GithubConfig;
use gate::Cancel;

const REFRESH_SECS: u64 = 60;
/// Error string `github_fetch` returns when a newer request took over. The UI
/// recognizes it and quietly keeps what it already has - it is not a failure.
pub const SUPERSEDED: &str = "superseded";
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

    /// `cfg` is unused deliberately: going through `GithubConfig::resolve()` is
    /// the point, because that is the exact call `fetch` makes below before
    /// reporting `NeedsAuth`, env fallbacks and all. A saved account row is
    /// neither sufficient (the Connectors page stores a label with no token
    /// happily) nor necessary (`GITHUB_TOKEN` covers a dev run with an empty
    /// config) - the token is, and it lives in the keychain rather than in
    /// `cfg`. The selected orgs stay out of it too: a token with none resolves,
    /// fetches, and reports a `Misconfigured` banner naming what to fix, which
    /// hiding the tab would hide as well.
    fn is_configured(&self, _cfg: &AppConfig) -> bool {
        GithubConfig::resolve().is_some()
    }

    /// The generic entry point: the first account, all orgs. The GitHub tab
    /// never takes this path - it drives its own per-account views through
    /// `fetch_account` - so this never preempts a fetch the user is waiting on:
    /// if one is already running it reuses the last snapshot rather than
    /// doubling the Search API spend to compute the same thing twice.
    async fn fetch(&self, ctx: &FetchCtx) -> Result<Snapshot, ConnectorError> {
        // Days are fixed to IST per the design (PRs near midnight are attributed
        // by IST datetime bounds). `ctx.timezone` is ignored for now.
        let Some(cfg) = GithubConfig::resolve() else {
            return Ok(Snapshot::needs_auth(i18n::t("github.needsAuth")));
        };

        let key = view_key(&cfg.label, None, &ctx.range);
        if let Some(snapshot) = gate::gate().fresh(&key) {
            return Ok(snapshot);
        }
        let Some(lease) = gate::gate().begin_if_idle(&key) else {
            return Ok(gate::gate()
                .last(&key)
                .unwrap_or_else(|| Snapshot::ok(vec![], Some(REFRESH_SECS))));
        };

        match run_fetch(&cfg, &ctx.range, lease.cancel()).await {
            Ok(snapshot) => {
                gate::gate().finish(&lease, &snapshot);
                Ok(snapshot)
            }
            Err(GithubError::Cancelled) => Ok(gate::gate()
                .last(&key)
                .unwrap_or_else(|| Snapshot::ok(vec![], Some(REFRESH_SECS)))),
            Err(GithubError::RateLimited { retry_after_secs }) => {
                Ok(rate_limited_snapshot(retry_after_secs))
            }
            Err(GithubError::Misconfigured(message)) => Err(ConnectorError::Misconfigured(message)),
            Err(e) => Err(ConnectorError::Other(e.to_string())),
        }
    }
}

/// Stable key for one dashboard view: account, org filter and date range - the
/// three things that change what is fetched. `\u{1f}` (unit separator) can
/// appear in none of them, so two views can never collide on the same key.
fn view_key(label: &str, org: Option<&str>, range: &DateRange) -> String {
    let range = range.normalized();
    format!(
        "{label}\u{1f}{org}\u{1f}{start}\u{1f}{end}",
        org = org.unwrap_or_default(),
        start = range.start,
        end = range.end,
    )
}

/// Fetch a specific account's dashboard, optionally scoped to a single org
/// (an org-filter sub-tab; `None` means all of the account's orgs), over the
/// selected day range. Any outcome the user should see comes back as a
/// `Snapshot` carrying the right `Health` (needsAuth / rateLimited /
/// misconfigured / error) so the UI renders a banner rather than a raw error;
/// the one `Err` is [`SUPERSEDED`], meaning a newer request took over.
///
/// Changing account, org or date range therefore cancels the fetch for the view
/// just left instead of stacking another one on top of it, and a snapshot
/// younger than the gate's TTL is reused unless `force` is set (the manual
/// Refresh button).
pub async fn fetch_account(
    label: String,
    org: Option<String>,
    range: DateRange,
    force: bool,
) -> Result<Snapshot, String> {
    let key = view_key(&label, org.as_deref(), &range);
    if !force {
        if let Some(snapshot) = gate::gate().fresh(&key) {
            return Ok(snapshot);
        }
    }
    let Some(cfg) = GithubConfig::for_account(&label, org.as_deref()) else {
        return Ok(Snapshot::needs_auth(i18n::t("github.needsAuth")));
    };

    let lease = gate::gate().begin(&key);
    let snapshot = match run_fetch(&cfg, &range, lease.cancel()).await {
        Ok(snapshot) => snapshot,
        Err(GithubError::Cancelled) => return Err(SUPERSEDED.to_string()),
        Err(GithubError::RateLimited { retry_after_secs }) => {
            rate_limited_snapshot(retry_after_secs)
        }
        Err(GithubError::Misconfigured(message)) => Snapshot {
            status: Health::Misconfigured { message },
            panels: vec![],
            fetched_at: Utc::now(),
            next_refresh_secs: None,
        },
        Err(e) => Snapshot {
            status: Health::Error {
                message: e.to_string(),
            },
            panels: vec![],
            fetched_at: Utc::now(),
            next_refresh_secs: None,
        },
    };
    // A result that lost the race is dropped by the gate, and the caller is told
    // it was superseded - painting it would be the out-of-order repaint that
    // made a refresh look like it had fetched nothing new.
    if lease.cancel().cancelled() {
        return Err(SUPERSEDED.to_string());
    }
    gate::gate().finish(&lease, &snapshot);
    Ok(snapshot)
}

/// One account's own numbers over `range`, for the widget: the PRs that
/// account's login merged and created, and the lines those merged PRs touched.
/// `label` picks the account (the widget's sub-tabs); `None` takes the first
/// configured one, as the dashboard's generic entry point does.
///
/// This is not the dashboard fetch narrowed down. The dashboard reports on the
/// account's configured orgs and counts every contributor in them; this asks
/// `author:<viewer>` instead, so it needs no org configuration, sees the user's
/// work wherever it happened, and costs two Search queries plus one GraphQL
/// enrichment rather than three per org plus the calendar. Cheap enough to run
/// from a window that only ever fetches when the user asks it to.
///
/// Every outcome comes back as a `Snapshot` carrying the right `Health`, so the
/// widget renders a one-line status instead of a raw error.
pub async fn fetch_mine(label: Option<String>, range: DateRange) -> Snapshot {
    // A label with no token in the keychain is a real state - an account row
    // saved before its token was pasted - and it belongs to that sub-tab alone,
    // so it must not fall back to another account's numbers under this one's
    // name.
    let resolved = match &label {
        Some(label) => GithubConfig::for_account(label, None),
        None => GithubConfig::resolve(),
    };
    let Some(cfg) = resolved else {
        return Snapshot::needs_auth(i18n::t("github.needsAuth"));
    };
    match run_fetch_mine(&cfg, &range).await {
        Ok(snapshot) => snapshot,
        Err(GithubError::RateLimited { retry_after_secs }) => {
            rate_limited_snapshot(retry_after_secs)
        }
        Err(GithubError::Misconfigured(message)) => Snapshot {
            status: Health::Misconfigured { message },
            panels: vec![],
            fetched_at: Utc::now(),
            next_refresh_secs: None,
        },
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

async fn run_fetch_mine(cfg: &GithubConfig, range: &DateRange) -> Result<Snapshot, GithubError> {
    let range = range.normalized();
    let bounds = range.ist_bounds();
    let client = GithubClient::new(&cfg.token)?;
    // Nothing to cancel: the widget runs one fetch at a time, on demand.
    let cancel = Cancel::none();

    // Whose PRs to count. Taken from the token rather than the account label,
    // which is a name the user typed and need not be their login.
    let login = client.viewer_profile().await?.login;

    let opened = client
        .search_issues(&format!("author:{login} type:pr created:{bounds}"), &cancel)
        .await?;
    let merged = client
        .search_issues(&format!("author:{login} type:pr merged:{bounds}"), &cancel)
        .await?;

    // Line counts live on the PR itself, so only the merged set is enriched -
    // the same merged-based definition the dashboard's line table uses.
    let refs: Vec<PrRef> = merged.items.iter().map(|it| it.pr_ref()).collect();
    let enriched = if refs.is_empty() {
        Vec::new()
    } else {
        client.enrich_prs(&refs, &cancel).await?
    };
    let additions = enriched.iter().map(|e| e.additions).sum();
    let deletions = enriched.iter().map(|e| e.deletions).sum();

    // The counts come from GitHub's own `total_count` rather than the rows it
    // served: one person's PRs cannot realistically pass the 1000-result cap,
    // but if they ever did, counting rows would quietly report the cap as the
    // answer. The line totals can only ever cover the rows we hold.
    Ok(Snapshot::ok(
        vec![aggregate::mine_stats(
            &login,
            opened.total,
            merged.total,
            additions,
            deletions,
        )],
        Some(REFRESH_SECS),
    ))
}

/// One dashboard fetch. `cancel` is polled between requests, so a fetch the user
/// has navigated away from stops after the call already in flight rather than
/// working through every remaining scope.
async fn run_fetch(
    cfg: &GithubConfig,
    range: &DateRange,
    cancel: &Cancel,
) -> Result<Snapshot, GithubError> {
    // Nothing to search. Caught here rather than falling through to an empty
    // dashboard, which reads as "no activity" instead of "not set up yet".
    if cfg.orgs.is_empty() {
        return Err(GithubError::Misconfigured(i18n::t("github.noScopes")));
    }

    let ist = range::ist();
    let range = range.normalized();
    let bounds = range.ist_bounds();
    // The same window as `bounds`, for the one question no search qualifier can
    // answer (see `search_scope`).
    let window = range.ist_window();
    let client = GithubClient::new(&cfg.token)?;

    let mut rollup = Rollup::default();
    // PRs deduped across every scope and every set, with per-set flags. Counting
    // happens here rather than per scope, so two overlapping scopes (an org plus
    // `author:me`, which the Connectors copy suggests pairing) cannot count the
    // same PR twice in a table sitting above an already-deduped PR list.
    let mut seen: HashMap<PrKey, SeenPr> = HashMap::new();
    // Scopes GitHub refused, kept aside so one bad entry cannot sink the others.
    let mut failed: Vec<String> = Vec::new();
    // The biggest query GitHub capped, so the dashboard can say the range is
    // wider than Search will serve instead of quietly reporting a slice of it.
    let mut truncated_total = 0u64;

    for entry in &cfg.orgs {
        let scope = scope_qualifier(entry);
        // `None` means the Search API answered 422: this scope is unsearchable
        // for this token, which the user can act on. It is specific to the
        // scope, so it is set aside rather than sinking the ones that do work.
        //
        // Every other failure propagates untouched. A 401 from a revoked token,
        // a 5xx, a DNS blip - none of those are attributable to this scope, and
        // most are transient, so they must stay `Error` ("we'll keep trying")
        // rather than being reported as a settings mistake that retrying cannot
        // fix. A rate limit is global too: trying the remaining scopes would
        // only burn more quota.
        let Some(sets) = search_scope(&client, &scope, &bounds, window, cancel).await? else {
            eprintln!("github: scope {scope} is unsearchable with this token");
            failed.push(scope);
            continue;
        };

        truncated_total = truncated_total.max(sets.truncated_total);

        // Fold every set into the deduped union with its flags.
        for it in sets.opened {
            upsert(&mut seen, it, Bucket::Created);
        }
        for it in sets.merged {
            upsert(&mut seen, it, Bucket::Merged);
        }
        for it in sets.closed {
            upsert(&mut seen, it, Bucket::Closed);
        }
    }

    // Nothing searchable at all: say which scopes were refused rather than
    // rendering an empty dashboard that looks like a quiet day.
    if failed.len() == cfg.orgs.len() {
        return Err(GithubError::Misconfigured(unsearchable_message(
            &cfg.token, &failed,
        )));
    }

    count_activity(&seen, cfg.filter_bots, &mut rollup);

    // Enrich every PR the dashboard will call merged - the ones merged inside
    // the range, which drive line contributions, plus the ones created inside it
    // and merged after it, which the PR list still labels "Merged". Enriching
    // only the first group left those rows claiming an outcome with "-" in the
    // +/- column, which reads as missing data rather than an out-of-range merge.
    let merged_refs: Vec<PrRef> = seen
        .values()
        .filter(|s| s.merged_in_range || s.item.merged_at.is_some())
        .map(|s| s.item.pr_ref())
        .collect();

    let enriched = if merged_refs.is_empty() {
        Vec::new()
    } else {
        client.enrich_prs(&merged_refs, cancel).await?
    };

    // Index enrichment by (nameWithOwner, number) for overlay onto the union.
    let mut enrich_by_key: HashMap<(String, u64), &EnrichedPr> = HashMap::new();
    for e in &enriched {
        enrich_by_key.insert((e.name_with_owner.clone(), e.number), e);
    }

    // Line contributions: PRs merged in the range, attributed to their author.
    // Driven off the deduped union rather than the enrichment results, so a PR
    // two scopes both matched contributes its lines once.
    for s in seen.values() {
        if !s.merged_in_range || filtered_out(s.item.author.as_deref(), cfg.filter_bots) {
            continue;
        }
        let name_with_owner = format!("{}/{}", s.item.owner, s.item.repo);
        let Some(e) = enrich_by_key.get(&(name_with_owner, s.item.number)) else {
            continue;
        };
        rollup.line_contribs.push(LineContrib {
            author: aggregate::author_label(s.item.author.as_deref()),
            additions: e.additions,
            deletions: e.deletions,
        });
    }

    // Build the union PR list for the range.
    for s in seen.values() {
        let author = s.item.author.clone();
        if filtered_out(author.as_deref(), cfg.filter_bots) {
            continue;
        }

        let name_with_owner = format!("{}/{}", s.item.owner, s.item.repo);
        let enriched = enrich_by_key.get(&(name_with_owner.clone(), s.item.number));

        let is_merged = s.merged_in_range || s.item.merged_at.is_some();
        let is_closed = s.closed_in_range || s.item.closed_at.is_some();
        let (state, at) = if is_merged {
            let merged_at = enriched
                .and_then(|e| e.merged_at)
                .or(s.item.merged_at)
                .or(s.item.closed_at)
                .or(s.item.created_at);
            (PrState::Merged, merged_at)
        } else if is_closed {
            (PrState::Closed, s.item.closed_at.or(s.item.created_at))
        } else if s.item.draft {
            // Getting this far already means neither merged nor closed, so the
            // draft flag can be trusted here - the same intersection the Drafts
            // column counts, which is why a row marked Draft is always one of
            // the PRs behind that number.
            (PrState::Draft, s.item.created_at)
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
    if cancel.cancelled() {
        return Err(GithubError::Cancelled);
    }
    let contributions = contributions_panels(&client).await;

    let mut panels = aggregate::build_panels(&rollup, ist, &range, contributions);
    // The range matched more PRs than Search will hand over, so the numbers
    // below are the newest slice of it. Silently rendering them was the worst
    // failure mode this connector had: a 30-day view could show six days of
    // data and look exactly like a complete one.
    if truncated_total > 0 {
        panels.insert(
            0,
            aggregate::results_truncated_note(&range.label(), truncated_total),
        );
    }
    // Some scopes worked and some did not: the numbers below are real but
    // incomplete, so say which scopes are missing rather than letting the
    // dashboard imply it covers everything.
    if !failed.is_empty() {
        panels.insert(0, aggregate::scopes_failed_note(&failed));
    }
    Ok(Snapshot::ok(panels, Some(REFRESH_SECS)))
}

/// The date-filtered PR sets for one scope over the selected range.
#[derive(Clone)]
struct ScopeSets {
    /// PRs created inside the range: the cohort every count column splits.
    opened: Vec<SearchItem>,
    /// PRs merged inside the range, whenever they were created. Feeds the line
    /// contributions table and the PR list, never the counts.
    merged: Vec<SearchItem>,
    /// PRs closed without merging inside the range, whenever they were created.
    /// Feeds the PR list only.
    closed: Vec<SearchItem>,
    /// `total_count` of the widest query GitHub capped, or 0 when every query
    /// fitted inside the 1000-result limit.
    truncated_total: u64,
}

/// Run one scope's searches over `bounds`. `scope` is an already-resolved search
/// qualifier (`org:acme`, `user:octocat`, `author:octocat`), never a raw config
/// entry. `Ok(None)` means the Search API answered 422 - GitHub's way of saying
/// this token cannot see the scope, whether because it does not exist or because
/// the token is not allowed to view it.
///
/// A poll that changes nothing costs one request, not three paginated ones:
/// `updated:{bounds}` is a superset of all three queries below - creating,
/// merging and closing a PR all touch it - so a probe of that superset
/// (`search_probe`) that matches the last one taken for this scope and range
/// proves every result set is still exactly what it was, and the cached sets
/// are handed back untouched.
///
/// Three queries, and deliberately no fourth for "still open": a merged PR is
/// closed too, so the `created` results that carry no `closed_at` are precisely
/// `created:{bounds} is:open` - verified against the live API. Search is
/// budgeted at 30 requests a minute and every query here paginates up to ten
/// pages, so a redundant set costs a third of the per-minute budget as soon as
/// the range is wider than a day.
async fn search_scope(
    client: &GithubClient,
    scope: &str,
    bounds: &str,
    window: (DateTime<Utc>, DateTime<Utc>),
    cancel: &Cancel,
) -> Result<Option<ScopeSets>, GithubError> {
    // `closed:` cannot answer the third question. Measured against the live API:
    // for one IST day on org:z-roworld, `closed:{bounds}` returns 99 and
    // `closed:{bounds} is:merged` returns the same 99, while
    // `closed:{bounds} is:unmerged` and `closed:{bounds} -is:merged` both return
    // 0 - on a day when five pull requests were genuinely closed without being
    // merged. GitHub indexes `closed:` off the merge, so the qualifier is
    // structurally incapable of finding an unmerged close, and the old query
    // could only ever report zero.
    //
    // Closing a pull request always touches it, so `updated:` over the same
    // window is a superset; the real closes are then picked out locally by
    // `closed_at`. It is a superset and not an equivalent - a PR closed earlier
    // and merely commented on today also matches - which is exactly why the
    // filter below is not optional.
    let queries = [
        format!("{scope} type:pr created:{bounds}"),
        format!("{scope} type:pr merged:{bounds}"),
        format!("{scope} type:pr is:closed is:unmerged updated:{bounds}"),
    ];

    // One request that answers "has anything in this window moved at all?".
    // A 422 here means the same thing it means below: the token cannot see
    // this scope.
    let probe = match client
        .search_probe(&format!("{scope} type:pr updated:{bounds}"))
        .await
    {
        Ok(p) => Some(p),
        Err(GithubError::Status { code: 422, .. }) => return Ok(None),
        Err(e) => return Err(e),
    };
    if let Some(probe) = &probe {
        if let Some(sets) = cache::unchanged_scope(client.token(), scope, bounds, probe) {
            return Ok(Some(sets));
        }
    }

    let mut results = Vec::with_capacity(queries.len());
    for q in queries {
        match client.search_issues(&q, cancel).await {
            Ok(r) => results.push(r),
            Err(GithubError::Status { code: 422, .. }) => return Ok(None),
            Err(e) => return Err(e),
        }
    }

    let truncated_total = results
        .iter()
        .filter(|r| r.truncated())
        .map(|r| r.total)
        .max()
        .unwrap_or(0);

    let mut results = results.into_iter().map(|r| r.items);
    let opened = results.next().unwrap_or_default();
    let merged = results.next().unwrap_or_default();
    // Narrow the `updated:` superset to the closes that actually happened in the
    // window. A PR with no `closed_at` cannot have been closed in it, so the
    // absent case drops rather than counting.
    let closed = results
        .next()
        .unwrap_or_default()
        .into_iter()
        .filter(|it| matches!(it.closed_at, Some(at) if at >= window.0 && at <= window.1))
        .collect();

    let sets = ScopeSets {
        opened,
        merged,
        closed,
        truncated_total,
    };
    // Store against the probe taken *before* the searches, never a fresher one:
    // anything that landed while they were in flight must still invalidate this.
    if let Some(probe) = probe {
        cache::store_scope(client.token(), scope, bounds, probe, &sets);
    }
    Ok(Some(sets))
}

/// Why every configured scope came back unsearchable, phrased for whoever has
/// to fix it.
///
/// A lone `org:` scope gets the token-kind-aware explanation below: the
/// third-party-application grant it talks about is an org policy, so it is only
/// meaningful there. Several failed scopes, or a personal `user:` / `author:`
/// one, get the generic list instead - naming each refused scope, since the
/// remedy may differ per entry.
fn unsearchable_message(token: &str, failed: &[String]) -> String {
    match failed {
        [only] => match only.strip_prefix("org:") {
            Some(org) => org_unsearchable_message(token, org),
            None => i18n::tf("github.scopesFailed", &[("scopes", only)]),
        },
        many => i18n::tf("github.scopesFailed", &[("scopes", &many.join(", "))]),
    }
}

/// Why an org came back unsearchable, phrased for whoever has to fix it.
///
/// The two token kinds fail this way for genuinely different reasons, and the
/// remedies do not overlap:
///
/// * A **Device Flow / OAuth App token** (`gho_`) is subject to the org's
///   third-party application access policy. Scopes are irrelevant here - even a
///   token holding `repo read:org read:user` sees the org as empty (`/user/orgs`
///   returns `[]`, the org's repo list returns `[]`) until an org owner grants
///   the OAuth App access. Search then reports 422.
/// * A **PAT** is not subject to that policy, so a 422 means the org name is
///   wrong, or the token lacks `repo` / SAML SSO authorization.
fn org_unsearchable_message(token: &str, org: &str) -> String {
    if token.starts_with("gho_") {
        i18n::tf(
            "github.orgNotGranted",
            &[("org", org), ("url", &device_flow::app_grant_url())],
        )
    } else {
        i18n::tf("github.orgUnsearchable", &[("org", org)])
    }
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

/// Identity of a pull request, and the key the union is deduped on.
type PrKey = (String, String, u64);

/// Which of a scope's three searches a PR came back from - one per date
/// qualifier, so `Merged` and `Closed` mean "that happened inside the range",
/// whenever the PR was opened.
#[derive(Debug, Clone, Copy)]
enum Bucket {
    Created,
    Merged,
    Closed,
}

/// A PR seen across one or more search sets, with the sets it appeared in.
struct SeenPr {
    item: SearchItem,
    created_in_range: bool,
    merged_in_range: bool,
    closed_in_range: bool,
}

fn upsert(seen: &mut HashMap<PrKey, SeenPr>, item: SearchItem, bucket: Bucket) {
    let entry = seen.entry(item.key()).or_insert_with(|| SeenPr {
        item: item.clone(),
        created_in_range: false,
        merged_in_range: false,
        closed_in_range: false,
    });
    match bucket {
        Bucket::Created => entry.created_in_range = true,
        Bucket::Merged => entry.merged_in_range = true,
        Bucket::Closed => entry.closed_in_range = true,
    }
}

/// Fill the per-contributor count maps the PR activity table renders.
///
/// Two properties make those columns trustworthy, and both come from counting
/// here - once, over the deduped union - rather than inside the scope loop:
///
/// * **Every PR counts once.** `seen` is keyed on owner/repo/number across every
///   configured scope, so a PR that two overlapping scopes both match (an org
///   plus `author:me`, a pairing the Connectors copy actively suggests) adds one
///   rather than two. Tallying per scope made the table disagree with the PR
///   list directly beneath it, which was already deduped this way.
/// * **One cohort, split by outcome.** The four outcome columns describe the PRs
///   *created* in the range, so a row always reconciles as
///   `created = merged + closed unmerged + still open`. Reading "merged" and
///   "closed unmerged" off the event-based `merged:` / `closed:` searches
///   instead swept in PRs opened long before the range: the outcome columns then
///   summed past Created, with nothing on screen to explain why.
///
/// The outcome split needs no extra request, because a search result already
/// carries `merged_at` and `closed_at`, and a merged PR is always closed too -
/// so "no `closed_at`" is exactly "still open right now".
///
/// Drafts is the fifth map and the only one outside that split: it counts the
/// still-open PRs whose author has not asked for review yet, so it is always a
/// subset of Still open and must never be added to a total.
fn count_activity(seen: &HashMap<PrKey, SeenPr>, filter_bots: bool, rollup: &mut Rollup) {
    for s in seen.values() {
        if filtered_out(s.item.author.as_deref(), filter_bots) {
            continue;
        }
        let author = aggregate::author_label(s.item.author.as_deref());
        let bump = |m: &mut HashMap<String, u64>| *m.entry(author.clone()).or_insert(0) += 1;

        // Three independent events. A pull request contributes to each column it
        // qualifies for and to no other, so one merged today after being opened
        // last week counts under Merged here and under Created in whichever
        // range covers last week.
        if s.created_in_range {
            bump(&mut rollup.opened);
        }
        if s.merged_in_range {
            bump(&mut rollup.merged);
        }
        if s.closed_in_range {
            bump(&mut rollup.closed);
        }

        // The one state reading. "Still open" has no event of its own - staying
        // open is the absence of one - so it is the opened-in-range cohort that
        // has not closed since, which is what makes it the only column tied to
        // Created.
        // `merged_at` is checked as well as `closed_at` even though a merged PR
        // is always closed too: if GitHub ever omits one, the fallback must be
        // to under-report Still open rather than to call a merged PR open.
        if s.created_in_range && s.item.closed_at.is_none() && s.item.merged_at.is_none() {
            bump(&mut rollup.open);
            // `draft` is a right-now flag that outlives the state it was set in:
            // GitHub keeps reporting it on pull requests that were later merged
            // or closed, so counting it on its own would tally ones already
            // sitting in the columns to the left. Intersecting it with still-open
            // is what makes Drafts a narrowing of that column rather than a
            // bucket beside it.
            if s.item.draft {
                bump(&mut rollup.drafts);
            }
        }
    }
}

/// Turn one configured scope entry into a GitHub search qualifier.
///
/// A bare name is an organization (`z-roworld` -> `org:z-roworld`), which is what
/// the Connectors UI has always written. An entry that already carries a
/// qualifier is passed through, and the three that are honored answer different
/// questions - picking the wrong one silently returns the wrong PRs rather than
/// erroring:
///
/// - `org:acme` - PRs in an organization's repos.
/// - `user:octocat` - PRs in a personal account's *own* repos. A personal
///   account is not an org, so `org:<login>` on one is a 422 from the Search
///   API rather than an empty result.
/// - `author:octocat` - PRs *written by* someone, in any repo. This is the one
///   that catches contributions to other people's projects: a PR opened against
///   an upstream repo lives in that repo, never in the author's account or
///   fork, so `user:` structurally cannot see it.
fn scope_qualifier(entry: &str) -> String {
    let entry = entry.trim();
    match entry.split_once(':') {
        Some((kind, name))
            if matches!(kind.trim(), "org" | "user" | "author") && !name.trim().is_empty() =>
        {
            format!("{}:{}", kind.trim(), name.trim())
        }
        _ => format!("org:{entry}"),
    }
}

/// Whether this PR is hidden by the app-wide "Filter bot authors" setting.
///
/// A PR with no author is never a bot: GitHub only drops the `user` field when
/// the account is gone, and erasing those PRs would leave the counts lower than
/// the rows visible right below them.
fn filtered_out(login: Option<&str>, filter_bots: bool) -> bool {
    filter_bots && login.map(is_bot).unwrap_or(false)
}

/// Obvious bot authors: any `...[bot]` account, plus dependabot, which posts
/// under a bare login on some installations.
///
/// Matched exactly rather than by prefix. `starts_with("dependabot")` also ate
/// human accounts such as `dependabot-mirror`, and a contributor vanishing from
/// every number on the dashboard is invisible until someone counts by hand.
fn is_bot(login: &str) -> bool {
    let l = login.to_ascii_lowercase();
    l.ends_with("[bot]") || l == "dependabot" || l == "dependabot-preview"
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
mod tests {
    use super::*;

    fn ts(s: &str) -> Option<DateTime<Utc>> {
        Some(
            DateTime::parse_from_rfc3339(s)
                .expect("test timestamp")
                .with_timezone(&Utc),
        )
    }

    /// A search result for `acme/api#{number}`, created inside the range and
    /// still open. Tests move `merged_at` / `closed_at` to pick an outcome.
    fn pr(number: u64, author: Option<&str>) -> SearchItem {
        SearchItem {
            number,
            title: format!("fix: thing {number}"),
            html_url: format!("https://github.com/acme/api/pull/{number}"),
            author: author.map(str::to_string),
            owner: "acme".into(),
            repo: "api".into(),
            created_at: ts("2026-08-07T04:00:00Z"),
            closed_at: None,
            updated_at: ts("2026-08-07T04:00:00Z"),
            merged_at: None,
            draft: false,
        }
    }

    /// The counts the PR activity table renders, bots filtered as they are by
    /// default.
    fn counts(seen: &HashMap<PrKey, SeenPr>) -> Rollup {
        let mut rollup = Rollup::default();
        count_activity(seen, true, &mut rollup);
        rollup
    }

    /// The double count this table shipped with: the tally ran once per
    /// configured scope, so a PR that both `org:acme` and `author:dev` matched
    /// added two to every column - while the PR list beneath it, deduped on
    /// owner/repo/number, showed the single row it really is.
    #[test]
    fn a_pr_matched_by_two_scopes_is_counted_once() {
        let mut seen = HashMap::new();
        for _ in 0..2 {
            upsert(&mut seen, pr(1, Some("dev")), Bucket::Created);
        }

        let r = counts(&seen);
        assert_eq!(r.opened.get("dev"), Some(&1));
        assert_eq!(r.open.get("dev"), Some(&1));
        assert_eq!(r.merged.get("dev"), None);
        assert_eq!(r.closed.get("dev"), None);
    }

    /// Created, Merged and Closed unmerged are three independent questions about
    /// the range, so each PR lands in exactly the columns whose event it had -
    /// and a row is not a partition of Created. Tying Merged to the created
    /// cohort instead reported 77 where github.com said 99 for the same day.
    #[test]
    fn each_event_column_counts_its_own_event() {
        // Opened before the range and merged inside it: Merged only.
        let mut old_merge = pr(1, Some("dev"));
        old_merge.created_at = ts("2026-07-01T04:00:00Z");
        old_merge.merged_at = ts("2026-08-07T09:00:00Z");
        old_merge.closed_at = ts("2026-08-07T09:00:00Z");
        // Opened before the range and abandoned inside it: Closed unmerged only.
        let mut old_close = pr(2, Some("dev"));
        old_close.created_at = ts("2026-07-01T04:00:00Z");
        old_close.closed_at = ts("2026-08-07T10:00:00Z");
        // Opened inside the range and still waiting: Created and Still open.
        let waiting = pr(3, Some("dev"));

        let mut seen = HashMap::new();
        upsert(&mut seen, old_merge, Bucket::Merged);
        upsert(&mut seen, old_close, Bucket::Closed);
        upsert(&mut seen, waiting, Bucket::Created);

        let r = counts(&seen);
        assert_eq!(
            r.merged["dev"], 1,
            "a merge in the range counts, however old"
        );
        assert_eq!(r.closed["dev"], 1, "so does a close in the range");
        assert_eq!(r.opened["dev"], 1, "only the one opened in the range");
        assert_eq!(r.open["dev"], 1);
    }

    /// The same PR opened AND merged inside the range is one event of each, so
    /// it appears under both columns rather than being made to choose. This is
    /// what stops Merged from being read as a slice of Created.
    #[test]
    fn a_pr_opened_and_merged_in_the_range_counts_in_both_columns() {
        let mut it = pr(1, Some("dev"));
        it.merged_at = ts("2026-08-07T09:00:00Z");
        it.closed_at = ts("2026-08-07T09:00:00Z");

        let mut seen = HashMap::new();
        upsert(&mut seen, it.clone(), Bucket::Created);
        upsert(&mut seen, it, Bucket::Merged);

        let r = counts(&seen);
        assert_eq!(r.opened["dev"], 1);
        assert_eq!(r.merged["dev"], 1);
        assert_eq!(
            r.open.get("dev"),
            None,
            "it closed, so it is not still open"
        );
        assert_eq!(r.closed.get("dev"), None, "merged is not closed-unmerged");
    }

    /// GitHub's `draft` flag survives whatever happened to the PR afterwards -
    /// a merged PR can still report `draft: true` - so counting the flag alone
    /// would tally PRs already shown under Merged and Closed unmerged. Drafts
    /// only ever means "still open and still a draft".
    #[test]
    fn drafts_count_only_the_prs_that_are_still_open() {
        let mut waiting = pr(1, Some("dev"));
        waiting.draft = true;
        let mut merged = pr(2, Some("dev"));
        merged.draft = true;
        merged.merged_at = ts("2026-08-07T09:00:00Z");
        merged.closed_at = ts("2026-08-07T09:00:00Z");
        let mut abandoned = pr(3, Some("dev"));
        abandoned.draft = true;
        abandoned.closed_at = ts("2026-08-07T10:00:00Z");
        let ready = pr(4, Some("dev"));

        let mut seen = HashMap::new();
        for it in [waiting, merged, abandoned, ready] {
            upsert(&mut seen, it, Bucket::Created);
        }

        let r = counts(&seen);
        assert_eq!(r.drafts["dev"], 1, "only the still-open draft counts");
        assert_eq!(r.open["dev"], 2, "the draft is still one of the open PRs");
        assert!(
            r.drafts["dev"] <= r.open["dev"],
            "Drafts narrows Still open, it never stands beside it"
        );
    }

    /// A contributor with no drafts must be absent from the map rather than
    /// present with a zero: the column renders `unwrap_or(&0)`, and a phantom
    /// key would be the one thing able to widen the contributor union.
    #[test]
    fn contributors_without_drafts_are_not_in_the_drafts_map() {
        let mut seen = HashMap::new();
        upsert(&mut seen, pr(1, Some("dev")), Bucket::Created);

        let r = counts(&seen);
        assert_eq!(r.open["dev"], 1);
        assert!(r.drafts.is_empty(), "{:?}", r.drafts);
    }

    /// "Still open" is the absence of a close, so it is the column most exposed
    /// to a missing timestamp: a merged PR whose `closed_at` GitHub omitted would
    /// otherwise be reported as waiting on a reviewer. `merged_at` is checked too
    /// so the failure mode is under-reporting, never calling a merged PR open.
    #[test]
    fn a_merged_pr_is_never_still_open_even_without_closed_at() {
        let mut odd = pr(1, Some("dev"));
        odd.merged_at = ts("2026-08-07T09:00:00Z");
        odd.closed_at = None;

        let mut seen = HashMap::new();
        upsert(&mut seen, odd, Bucket::Created);

        let r = counts(&seen);
        assert_eq!(r.opened["dev"], 1, "it was still opened in the range");
        assert!(r.open.is_empty(), "{:?}", r.open);
    }

    /// "Filter bot authors" was a dead setting: bots were dropped whatever it
    /// said, so a user who turned it off saw no change and no explanation.
    #[test]
    fn the_bot_filter_follows_the_setting() {
        let mut seen = HashMap::new();
        upsert(&mut seen, pr(1, Some("dependabot[bot]")), Bucket::Created);
        upsert(&mut seen, pr(2, Some("dev")), Bucket::Created);

        let mut filtered = Rollup::default();
        count_activity(&seen, true, &mut filtered);
        assert_eq!(filtered.opened.len(), 1);
        assert!(!filtered.opened.contains_key("dependabot[bot]"));

        let mut kept = Rollup::default();
        count_activity(&seen, false, &mut kept);
        assert_eq!(kept.opened.get("dependabot[bot]"), Some(&1));
        assert_eq!(kept.opened.get("dev"), Some(&1));
    }

    /// The prefix match this used to do erased any human whose login merely
    /// began "dependabot", and a contributor missing from every number is
    /// invisible until somebody counts by hand.
    #[test]
    fn only_real_bots_are_filtered() {
        assert!(is_bot("dependabot[bot]"));
        assert!(is_bot("renovate[bot]"));
        assert!(is_bot("dependabot"));
        assert!(is_bot("dependabot-preview"));
        assert!(!is_bot("dependabot-mirror"));
        assert!(!is_bot("robotnik"));
    }

    /// A PR whose author's account is gone is still a PR: it used to be skipped
    /// by the counts while the list below rendered a row for it, so the stat
    /// cards could read lower than the rows beneath them for no visible reason.
    #[test]
    fn authorless_prs_are_counted_under_one_translated_label() {
        let mut seen = HashMap::new();
        upsert(&mut seen, pr(1, None), Bucket::Created);

        let label = aggregate::author_label(None);
        assert_ne!(label, "github.unknownAuthor", "untranslated: {label}");

        let r = counts(&seen);
        assert_eq!(r.opened.get(&label), Some(&1));
        assert_eq!(r.open.get(&label), Some(&1));
    }

    /// The truncation warning is the only thing standing between a capped range
    /// and numbers that look complete, so its copy must resolve and substitute.
    #[test]
    fn truncation_copy_resolves() {
        let title = i18n::t("github.truncatedTitle");
        assert_ne!(title, "github.truncatedTitle");

        let body = i18n::tf(
            "github.truncated",
            &[
                ("range", "Last 7 days"),
                ("total", "1,758"),
                ("cap", "1,000"),
            ],
        );
        assert!(body.contains("Last 7 days"), "{body}");
        assert!(body.contains("1,758"), "{body}");
        assert!(!body.contains('{'), "unsubstituted: {body}");
    }

    /// The stored token decides the tab, not the config file. Asking the same
    /// resolver `fetch` asks is what keeps the two from disagreeing, and it is
    /// why an account row saved without a token - which the Connectors page
    /// allows - cannot light the tab up on its own.
    #[test]
    fn github_is_configured_follows_the_token_not_the_config() {
        let connector = GithubConnector::new();
        let mut cfg = AppConfig::default();
        assert_eq!(
            connector.is_configured(&cfg),
            GithubConfig::resolve().is_some(),
            "the answer stopped coming from the resolver `fetch` uses",
        );

        cfg.github
            .accounts
            .push(crate::engine::config::GithubAccount {
                label: "fastdash-test-account-with-no-token".into(),
                orgs: vec!["acme".into()],
            });
        assert_eq!(
            connector.is_configured(&cfg),
            GithubConfig::resolve().is_some(),
            "a tokenless account row moved the answer",
        );
    }

    /// An OAuth (Device Flow) token gets the org-grant explanation; a PAT, which
    /// the third-party app policy does not apply to, gets the generic one.
    #[test]
    fn unsearchable_message_depends_on_token_kind() {
        let oauth = org_unsearchable_message("gho_abc123", "z-roworld");
        assert!(oauth.contains("z-roworld"), "{oauth}");
        assert!(oauth.contains("third-party OAuth apps"), "{oauth}");

        let pat = org_unsearchable_message("ghp_abc123", "z-roworld");
        assert!(pat.contains("z-roworld"), "{pat}");
        assert!(!pat.contains("third-party OAuth apps"), "{pat}");
    }

    /// Both messages must resolve to real catalog entries, never the bare key.
    #[test]
    fn unsearchable_messages_are_translated() {
        for token in ["gho_x", "ghp_x"] {
            let msg = org_unsearchable_message(token, "acme");
            assert!(!msg.starts_with("github."), "untranslated: {msg}");
            assert!(!msg.contains("{org}"), "unsubstituted: {msg}");
            assert!(!msg.contains("{url}"), "unsubstituted: {msg}");
        }
    }

    /// A lone org scope keeps the richer org-grant copy; a personal scope, which
    /// no org policy applies to, must not claim an org is withholding access.
    #[test]
    fn lone_org_scope_keeps_the_org_explanation() {
        let org = unsearchable_message("gho_abc123", &["org:z-roworld".to_string()]);
        assert!(org.contains("third-party OAuth apps"), "{org}");

        let user = unsearchable_message("gho_abc123", &["user:octocat".to_string()]);
        assert!(!user.contains("third-party OAuth apps"), "{user}");
        assert!(user.contains("user:octocat"), "{user}");
    }

    /// Several refused scopes are all named, so the user knows which entries to
    /// fix rather than just the first one.
    #[test]
    fn multiple_failed_scopes_are_all_listed() {
        let msg = unsearchable_message(
            "ghp_x",
            &["org:acme".to_string(), "user:octocat".to_string()],
        );
        assert!(msg.contains("org:acme"), "{msg}");
        assert!(msg.contains("user:octocat"), "{msg}");
    }

    #[test]
    fn bare_name_is_an_org() {
        assert_eq!(scope_qualifier("z-roworld"), "org:z-roworld");
    }

    #[test]
    fn explicit_qualifiers_pass_through() {
        assert_eq!(scope_qualifier("user:octocat"), "user:octocat");
        assert_eq!(scope_qualifier("org:z-roworld"), "org:z-roworld");
        assert_eq!(scope_qualifier("author:octocat"), "author:octocat");
    }

    #[test]
    fn surrounding_whitespace_is_trimmed() {
        assert_eq!(scope_qualifier("  acme "), "org:acme");
        assert_eq!(scope_qualifier(" user: octocat "), "user:octocat");
    }

    /// An unknown qualifier is not silently honored: treating `repo:a/b` as a
    /// raw pass-through would let any search qualifier be injected through the
    /// org field, so it falls back to the org reading instead.
    #[test]
    fn unknown_qualifier_is_not_passed_through() {
        assert_eq!(scope_qualifier("repo:acme/widget"), "org:repo:acme/widget");
    }

    #[test]
    fn empty_name_after_qualifier_falls_back() {
        assert_eq!(scope_qualifier("user:"), "org:user:");
    }

    /// `i18n::t` returns the key itself when a string is missing, so a typo in a
    /// locale key ships silently as `github.scopesFailed` in the user's banner.
    /// These assert the keys resolve and that the placeholder is substituted.
    #[test]
    fn misconfigured_copy_resolves() {
        let no_scopes = i18n::t("github.noScopes");
        assert_ne!(no_scopes, "github.noScopes");
        assert!(!no_scopes.is_empty());

        let failed = i18n::tf("github.scopesFailed", &[("scopes", "org:Personal")]);
        assert_ne!(failed, "github.scopesFailed");
        assert!(failed.contains("org:Personal"));
        assert!(!failed.contains("{scopes}"));
    }

    #[test]
    fn partial_note_copy_resolves() {
        let title = i18n::t("github.scopesFailedTitle");
        assert_ne!(title, "github.scopesFailedTitle");

        let body = i18n::tf("github.scopesPartial", &[("scopes", "org:Personal")]);
        assert_ne!(body, "github.scopesPartial");
        assert!(body.contains("org:Personal"));
        assert!(!body.contains("{scopes}"));
    }
}

#[cfg(test)]
mod live_test {
    use super::*;

    /// What each count column reports for one scope and day, against the real
    /// API, next to the answer github.com gives for the same question. This is
    /// the check that catches a column quietly meaning something else: the unit
    /// tests can only prove the code does what it says, never that what it says
    /// is what GitHub reports. Run with:
    ///   GITHUB_TOKEN=<token> FASTDASH_DIAG_SCOPE=org:acme \
    ///     cargo test --lib github::live_test::counts_diag -- --ignored --nocapture
    /// `FASTDASH_DIAG_DAY` (YYYY-MM-DD, IST) defaults to today.
    #[ignore = "hits the live GitHub API; run with --ignored and GITHUB_TOKEN set"]
    #[tokio::test]
    async fn counts_diag() {
        let token = std::env::var("GITHUB_TOKEN").expect("GITHUB_TOKEN");
        let scope = std::env::var("FASTDASH_DIAG_SCOPE").expect("FASTDASH_DIAG_SCOPE");
        let day = std::env::var("FASTDASH_DIAG_DAY")
            .ok()
            .and_then(|d| NaiveDate::parse_from_str(&d, "%Y-%m-%d").ok())
            .unwrap_or_else(range::today_ist);
        let range = DateRange {
            start: day,
            end: day,
        };
        let bounds = range.ist_bounds();
        let client = GithubClient::new(&token).expect("client");

        let sets = search_scope(
            &client,
            &scope,
            &bounds,
            range.ist_window(),
            &Cancel::none(),
        )
        .await
        .expect("search failed")
        .expect("scope is unsearchable with this token");

        let mut seen = HashMap::new();
        for it in sets.opened {
            upsert(&mut seen, it, Bucket::Created);
        }
        for it in sets.merged {
            upsert(&mut seen, it, Bucket::Merged);
        }
        for it in sets.closed {
            upsert(&mut seen, it, Bucket::Closed);
        }
        let mut rollup = Rollup::default();
        count_activity(&seen, true, &mut rollup);

        let total = |m: &HashMap<String, u64>| -> u64 { m.values().sum() };
        // Bots are filtered above, so a small gap against github.com is expected
        // here and is not the drift this diagnostic is looking for.
        async fn truth(client: &GithubClient, scope: &str, q: &str) -> u64 {
            client
                .search_issues(&format!("{scope} type:pr {q}"), &Cancel::none())
                .await
                .map(|r| r.total)
                .unwrap_or(0)
        }

        eprintln!("scope {scope}  day {day} (IST)");
        eprintln!(
            "  Created         {:>5}   github.com: {:>5}",
            total(&rollup.opened),
            truth(&client, &scope, &format!("created:{bounds}")).await
        );
        eprintln!(
            "  Merged          {:>5}   github.com: {:>5}",
            total(&rollup.merged),
            truth(&client, &scope, &format!("merged:{bounds}")).await
        );
        eprintln!(
            "  Closed unmerged {:>5}   github.com: (no query can answer this)",
            total(&rollup.closed),
        );
        eprintln!(
            "  Still open      {:>5}   github.com: {:>5}",
            total(&rollup.open),
            truth(&client, &scope, &format!("created:{bounds} is:open")).await
        );
        eprintln!(
            "  Drafts          {:>5}   github.com: {:>5}",
            total(&rollup.drafts),
            truth(
                &client,
                &scope,
                &format!("created:{bounds} is:open draft:true")
            )
            .await
        );
    }

    /// Switching view while a fetch is running must cancel the one being left,
    /// against the real API - the whole point being that the abandoned view
    /// stops spending the Search budget and can never paint over the view the
    /// user actually switched to. Ignored by default; run with:
    ///   GITHUB_TOKEN=<token> cargo test --lib github::live_test::switching_views -- --ignored --nocapture
    #[ignore = "hits the live GitHub API; run with --ignored and GITHUB_TOKEN set"]
    #[tokio::test]
    async fn switching_views_cancels_the_fetch_left_behind() {
        let cfg = GithubConfig::resolve().expect("set GITHUB_TOKEN for the live test");
        let scope = cfg.orgs.first().cloned().expect("a scope to narrow to");

        // The view the user leaves, then the one they land on - started while
        // the first is still in flight, exactly as a fast sub-tab click does.
        let left = tokio::spawn(fetch_account(
            cfg.label.clone(),
            None,
            DateRange::today(),
            true,
        ));
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let landed = fetch_account(cfg.label.clone(), Some(scope), DateRange::today(), true).await;

        assert_eq!(
            left.await.expect("task panicked").err().as_deref(),
            Some(SUPERSEDED),
            "the abandoned view must stand down instead of returning data"
        );
        assert!(landed.is_ok(), "the view switched to must still load");
    }

    /// Live smoke test against the real GitHub API. Ignored by default; run with:
    ///   GITHUB_TOKEN=<token> cargo test -p fastdash github::live_test -- --ignored --nocapture
    /// (get a work-account token via `gh auth token -u saheer-zro`).
    #[ignore = "hits the live GitHub API; run with --ignored and GITHUB_TOKEN set"]
    #[tokio::test]
    async fn live_fetch_smoke() {
        let cfg = GithubConfig::resolve().expect("set GITHUB_TOKEN for the live test");
        let range = DateRange::today();
        eprintln!("orgs: {:?}", cfg.orgs);
        eprintln!("bounds: {}", range.ist_bounds());

        let snapshot = run_fetch(&cfg, &range, &Cancel::none())
            .await
            .expect("fetch failed");
        eprintln!("status: {:?}", snapshot.status);
        eprintln!("panels: {}", snapshot.panels.len());
        let json = serde_json::to_string_pretty(&snapshot.panels).unwrap();
        eprintln!("{json}");
    }

    /// What a second poll of the same dashboard actually costs. Fetches one
    /// org twice, back to back, and reports how much of the second fetch was
    /// answered without asking GitHub for data: scopes whose one-request probe
    /// proved nothing in the window had moved, so the paginated searches were
    /// skipped, and PRs whose `updated_at` had not moved, so GraphQL was never
    /// asked about them.
    /// Run with:
    ///   GITHUB_TOKEN=<token> FASTDASH_DIAG_SCOPE=org:acme     ///     cargo test --lib github::live_test::polling_cost_diag -- --ignored --nocapture
    #[ignore = "hits the live GitHub API; run with --ignored and GITHUB_TOKEN set"]
    #[tokio::test]
    async fn polling_cost_diag() {
        let token = std::env::var("GITHUB_TOKEN").expect("GITHUB_TOKEN");
        let scope = std::env::var("FASTDASH_DIAG_SCOPE").expect("FASTDASH_DIAG_SCOPE");
        let day = range::today_ist();
        let range = DateRange {
            start: day,
            end: day,
        };
        let bounds = range.ist_bounds();
        let client = GithubClient::new(&token).expect("client");
        let window = range.ist_window();

        let poll = |label: &'static str| {
            let client = &client;
            let scope = scope.clone();
            let bounds = bounds.clone();
            async move {
                let before = cache::savings();
                let sets = search_scope(client, &scope, &bounds, window, &Cancel::none())
                    .await
                    .expect("search failed")
                    .expect("scope is unsearchable with this token");
                let refs: Vec<PrRef> = sets.merged.iter().map(|it| it.pr_ref()).collect();
                if !refs.is_empty() {
                    client
                        .enrich_prs(&refs, &Cancel::none())
                        .await
                        .expect("enrich failed");
                }
                let after = cache::savings();
                eprintln!(
                    "{label}: scopes {} reused / {} searched   PRs {} cached / {} asked",
                    after.0 - before.0,
                    after.1 - before.1,
                    after.2 - before.2,
                    after.3 - before.3,
                );
                after
            }
        };

        poll("first  poll").await;
        let before_second = cache::savings();
        poll("second poll").await;
        let after = cache::savings();

        assert!(
            after.0 > before_second.0,
            "the second poll re-ran the searches; the change probe is not gating them"
        );
        assert_eq!(
            after.1, before_second.1,
            "nothing changed between the polls, so nothing should have been re-searched"
        );
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
            .search_issues(
                &format!("author:{} type:pr", profile.login),
                &Cancel::none(),
            )
            .await
            .map(|r| r.items.len())
            .unwrap_or(0);
        eprintln!(
            "PRs authored by {} (search, all time): {authored}",
            profile.login
        );
    }
}
