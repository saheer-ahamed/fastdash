//! GitHub connector.
//!
//! Per selected org, uses the REST Search API for the date-filtered PR sets
//! (opened / merged / closed-without-merge / still-open over the selected IST
//! day range), then a single batched GraphQL enrichment for
//! additions/deletions/state on the MERGED set. Emits a `StatCards` header plus
//! three tables: PR counts per contributor, line contributions per contributor
//! (based on PRs MERGED in the range), and the PR list with repos.
//!
//! The range comes from the UI's date filter and is applied the way GitHub
//! itself filters - as `created:` / `merged:` / `closed:` qualifiers on the
//! Search API - so the numbers match what the same query shows on github.com.
//! It defaults to today. The contribution heatmap is deliberately exempt: it is
//! GitHub's own rolling calendar, not a range report.
//!
//! Supports multiple accounts (work `saheer-zro`, personal `saheer-ahamed`),
//! each with its own PAT in the OS keychain. The scheduler's `fetch` renders the
//! first account (all orgs) for the sidebar status dot; the UI drives per-account
//! and per-org sub-tabs through `fetch_account` (see `ipc::github_fetch`).

mod aggregate;
mod client;
mod config;
pub mod device_flow;
mod gate;

use std::collections::HashMap;

use async_trait::async_trait;
use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};

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

    /// The scheduler's periodic tick, which only feeds the sidebar status dot -
    /// the GitHub tab drives its own per-account views through `fetch_account`.
    /// It therefore never preempts a fetch the user is waiting on: if one is
    /// already running it reuses the last snapshot rather than doubling the
    /// Search API spend to compute the same thing twice.
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
    let client = GithubClient::new(&cfg.token)?;

    let mut rollup = Rollup::default();
    // PRs deduped across all four sets, with per-set outcome flags.
    let mut seen: HashMap<(String, String, u64), SeenPr> = HashMap::new();
    // Scopes GitHub refused, kept aside so one bad entry cannot sink the others.
    let mut failed: Vec<String> = Vec::new();

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
        let Some(sets) = search_scope(&client, &scope, &bounds, cancel).await? else {
            eprintln!("github: scope {scope} is unsearchable with this token");
            failed.push(scope);
            continue;
        };

        // Independent per-contributor counts (a PR may fall in several buckets).
        count_authors(&sets.opened, &mut rollup.opened);
        count_authors(&sets.merged, &mut rollup.merged);
        count_authors(&sets.closed, &mut rollup.closed);
        count_authors(&sets.still_open, &mut rollup.open);

        // Fold every set into the deduped union with outcome flags.
        for it in sets.opened {
            upsert(&mut seen, it, false, false, false);
        }
        for it in sets.merged {
            upsert(&mut seen, it, true, false, false);
        }
        for it in sets.closed {
            upsert(&mut seen, it, false, true, false);
        }
        for it in sets.still_open {
            upsert(&mut seen, it, false, false, true);
        }
    }

    // Nothing searchable at all: say which scopes were refused rather than
    // rendering an empty dashboard that looks like a quiet day.
    if failed.len() == cfg.orgs.len() {
        return Err(GithubError::Misconfigured(unsearchable_message(
            &cfg.token, &failed,
        )));
    }

    // Enrich only the MERGED set (line contributions are merged-based).
    let merged_refs: Vec<PrRef> = seen
        .values()
        .filter(|s| s.merged)
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

    // Build the union PR list for the range.
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
    if cancel.cancelled() {
        return Err(GithubError::Cancelled);
    }
    let contributions = contributions_panels(&client).await;

    let mut panels = aggregate::build_panels(&rollup, ist, &range, contributions);
    // Some scopes worked and some did not: the numbers below are real but
    // incomplete, so say which scopes are missing rather than letting the
    // dashboard imply it covers everything.
    if !failed.is_empty() {
        panels.insert(0, aggregate::scopes_failed_note(&failed));
    }
    Ok(Snapshot::ok(panels, Some(REFRESH_SECS)))
}

/// The four date-filtered PR sets for one scope over the selected range
/// (`still_open` is derived from `opened`, not searched separately).
struct ScopeSets {
    opened: Vec<SearchItem>,
    merged: Vec<SearchItem>,
    closed: Vec<SearchItem>,
    still_open: Vec<SearchItem>,
}

/// Run one scope's searches over `bounds`. `scope` is an already-resolved search
/// qualifier (`org:acme`, `user:octocat`, `author:octocat`), never a raw config
/// entry. `Ok(None)` means the Search API answered 422 - GitHub's way of saying
/// this token cannot see the scope, whether because it does not exist or because
/// the token is not allowed to view it.
///
/// Only three queries are issued: "still open" is `created:{bounds} is:open`,
/// which is exactly the subset of the `created` results that carry no
/// `closed_at`, so it is derived locally. Search is budgeted at 30 requests a
/// minute and every query here paginates, so dropping a whole redundant set
/// matters as soon as the range is wider than a day.
async fn search_scope(
    client: &GithubClient,
    scope: &str,
    bounds: &str,
    cancel: &Cancel,
) -> Result<Option<ScopeSets>, GithubError> {
    let queries = [
        format!("{scope} type:pr created:{bounds}"),
        format!("{scope} type:pr merged:{bounds}"),
        format!("{scope} type:pr closed:{bounds} is:unmerged"),
    ];

    let mut sets = Vec::with_capacity(queries.len());
    for q in queries {
        match client.search_issues(&q, cancel).await {
            Ok(items) => sets.push(items),
            Err(GithubError::Status { code: 422, .. }) => return Ok(None),
            Err(e) => return Err(e),
        }
    }

    let mut sets = sets.into_iter();
    let opened = sets.next().unwrap_or_default();
    // A merged PR is closed too, so "no closed_at" is precisely `is:open`.
    let still_open: Vec<SearchItem> = opened
        .iter()
        .filter(|it| it.closed_at.is_none())
        .cloned()
        .collect();

    Ok(Some(ScopeSets {
        opened,
        merged: sets.next().unwrap_or_default(),
        closed: sets.next().unwrap_or_default(),
        still_open,
    }))
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
mod tests {
    use super::*;

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
            .map(|v| v.len())
            .unwrap_or(0);
        eprintln!(
            "PRs authored by {} (search, all time): {authored}",
            profile.login
        );
    }
}
