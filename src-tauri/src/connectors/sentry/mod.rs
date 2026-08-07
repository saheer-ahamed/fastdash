//! Sentry connector.
//!
//! Per configured connection, pulls the organization issue stream
//! (`is:unresolved`) over the UI's date range and emits, per organization, a
//! `StatCards` header, an events-by-project breakdown, and the issue table.
//! Every number is derived from that one paginated endpoint, so a dashboard
//! costs one request per organization rather than one per project.
//!
//! The range comes from the UI's date filter and is sent as Sentry's
//! `start`/`end` window in UTC, so "Events" counts what happened inside the
//! selected days rather than the issue's lifetime total. It defaults to today.
//! `is:unresolved` is a state filter, not a date one: an issue that first fired
//! months ago but is still erroring today belongs on a dashboard of what needs
//! attention, and the "New issues" stat is what separates the two.
//!
//! Owned modules:
//!   - `client`    HTTP client for `/api/0`, pagination, error mapping
//!   - `config`    keychain + env resolution of the configured connections
//!   - `aggregate` rollups and `Panel` construction

mod aggregate;
mod client;
mod config;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::engine::config::AppConfig;
use crate::engine::connector::{Connector, ConnectorError, ConnectorMeta, FetchCtx, Snapshot};
use crate::engine::i18n;
use crate::engine::panel::Panel;
use crate::engine::range::{self, DateRange};

use aggregate::{OrgReport, Window};
use client::{SentryClient, SentryError};
use config::SentryConnection;

/// Sentry aggregates on ingest, so a minute-by-minute poll would mostly redraw
/// the same numbers; two minutes keeps the dashboard live without the churn.
const REFRESH_SECS: u64 = 120;

/// What the dashboard reports on: issues that are still someone's problem.
const QUERY: &str = "is:unresolved";

/// Prefix of a Sentry **organization** auth token. Those are built for CI
/// (releases, source maps) and carry no `event:read`, so the issue stream
/// answers 403 no matter which organization is asked for - worth naming,
/// because the generic "check your scopes" advice cannot be acted on when the
/// token kind has no such scope to grant.
const ORG_TOKEN_PREFIX: &str = "sntrys_";

pub struct SentryConnector;

impl SentryConnector {
    pub fn new() -> Self {
        SentryConnector
    }
}

#[async_trait]
impl Connector for SentryConnector {
    fn meta(&self) -> ConnectorMeta {
        ConnectorMeta {
            id: "sentry".into(),
            name: "Sentry".into(),
            icon: "sentry".into(),
            default_refresh_secs: REFRESH_SECS,
        }
    }

    /// The same guard `fetch` opens with, for the same reason: a connection is
    /// an account that has both a label and a resolvable token, so `cfg` alone
    /// cannot answer it - the token lives in the keychain, and a half-filled
    /// account row is one the user is still typing. `connections()` also carries
    /// the `SENTRY_AUTH_TOKEN` fallback, which a config-derived answer would
    /// miss on a dev run.
    fn is_configured(&self, _cfg: &AppConfig) -> bool {
        !config::connections().is_empty()
    }

    async fn fetch(&self, ctx: &FetchCtx) -> Result<Snapshot, ConnectorError> {
        // Days are fixed to IST per the design, matching the other connectors.
        // `ctx.timezone` is ignored for now.
        let connections = config::connections();
        if connections.is_empty() {
            return Ok(Snapshot::needs_auth(i18n::t("sentry.needsAuth")));
        }

        let range = ctx.range.normalized();
        let window = utc_window(&range);

        let mut reports: Vec<OrgReport> = Vec::new();
        // Organizations Sentry refused, kept aside so one bad slug cannot sink
        // the ones that work.
        let mut failed: Vec<String> = Vec::new();

        for conn in &connections {
            let client =
                SentryClient::new(&conn.base_url, &conn.token).map_err(|e| map_error(conn, e))?;

            let orgs = match resolve_orgs(&client, conn).await {
                Ok(orgs) => orgs,
                Err(e) => return Err(map_error(conn, e)),
            };
            if orgs.is_empty() {
                return Err(ConnectorError::Misconfigured(i18n::tf(
                    "sentry.noOrgs",
                    &[("label", &conn.label)],
                )));
            }

            for org in orgs {
                match client
                    .issues(&org, QUERY, &fmt(window.start), &fmt(window.end))
                    .await
                {
                    Ok(page) => reports.push(OrgReport {
                        account: conn.label.clone(),
                        org,
                        issues: page.issues,
                        truncated: page.truncated,
                    }),
                    // A slug that does not resolve is specific to this entry
                    // and the user can act on it, so it is set aside. Every
                    // other failure - a revoked token, a 5xx, a DNS blip - is
                    // not attributable to the organization and must sink the
                    // fetch rather than be reported as a settings mistake.
                    Err(SentryError::NotFound) => {
                        eprintln!("sentry: organization {org} not found for this token");
                        failed.push(org);
                    }
                    Err(e) => return Err(map_error(conn, e)),
                }
            }
        }

        // Nothing resolved at all: say which slugs were refused rather than
        // rendering an empty dashboard that looks like a quiet day.
        if reports.is_empty() && !failed.is_empty() {
            return Err(ConnectorError::Misconfigured(i18n::tf(
                "sentry.orgsFailed",
                &[("orgs", &failed.join(", "))],
            )));
        }

        let mut panels: Vec<Panel> = Vec::new();
        // Some organizations worked and some did not: the numbers below are
        // real but incomplete, so say which are missing rather than letting the
        // dashboard imply it covers everything.
        if !failed.is_empty() {
            panels.push(aggregate::orgs_failed_note(&failed));
        }
        panels.extend(aggregate::build_panels(&reports, &range, window));
        Ok(Snapshot::ok(panels, Some(REFRESH_SECS)))
    }
}

/// The organizations to report on: the configured slugs, or - when the account
/// lists none - every organization the token can see, so a fresh connection
/// needs a token and nothing else.
async fn resolve_orgs(
    client: &SentryClient,
    conn: &SentryConnection,
) -> Result<Vec<String>, SentryError> {
    if !conn.organizations.is_empty() {
        return Ok(conn.organizations.clone());
    }
    Ok(client
        .organizations()
        .await?
        .into_iter()
        .map(|o| o.slug)
        .collect())
}

/// The selected days as an absolute UTC window. The IST day boundary is the one
/// every connector uses, so "today" means the same thing across the dashboard
/// however Sentry's own organization timezone is configured.
fn utc_window(range: &DateRange) -> Window {
    let tz = range::ist();
    let at = |day: chrono::NaiveDate, h, m, s| -> DateTime<Utc> {
        day.and_hms_opt(h, m, s)
            .expect("a literal wall-clock time is valid")
            .and_local_timezone(tz)
            .single()
            .expect("IST is a fixed offset, so no local time is ambiguous")
            .with_timezone(&Utc)
    };
    Window {
        start: at(range.start, 0, 0, 0),
        end: at(range.end, 23, 59, 59),
        tz,
    }
}

/// Sentry wants a naive timestamp alongside `utc=true`, not an offset.
fn fmt(at: DateTime<Utc>) -> String {
    at.format("%Y-%m-%dT%H:%M:%S").to_string()
}

/// Turn a client error into the health the UI should show.
///
/// The distinction that matters is retryable vs. not: `NeedsAuth` and
/// `Misconfigured` tell the user to go change something, while `Other` reads as
/// "transient, we'll keep trying". Getting it wrong either nags about a blip or
/// silently retries a token that will never work again.
fn map_error(conn: &SentryConnection, err: SentryError) -> ConnectorError {
    match err {
        SentryError::Unauthorized(_) => {
            ConnectorError::Auth(i18n::tf("sentry.tokenRejected", &[("label", &conn.label)]))
        }
        SentryError::Forbidden(_) => ConnectorError::Misconfigured(forbidden_message(conn)),
        SentryError::RateLimited { .. } => ConnectorError::RateLimited,
        SentryError::BadUrl(url) => ConnectorError::Misconfigured(i18n::tf(
            "sentry.badUrl",
            &[("url", &url), ("label", &conn.label)],
        )),
        // A 404 reaches here only from the organization discovery call, which
        // means the base URL points somewhere that is not a Sentry API.
        SentryError::NotFound => ConnectorError::Misconfigured(i18n::tf(
            "sentry.badUrl",
            &[("url", &conn.base_url), ("label", &conn.label)],
        )),
        other => ConnectorError::Other(other.to_string()),
    }
}

/// Why Sentry refused, phrased for whoever has to fix it. An organization auth
/// token cannot be fixed by granting a scope - it has none to grant - so it
/// gets its own copy rather than advice that leads nowhere.
fn forbidden_message(conn: &SentryConnection) -> String {
    if conn.token.starts_with(ORG_TOKEN_PREFIX) {
        i18n::tf("sentry.orgTokenKind", &[("label", &conn.label)])
    } else {
        i18n::tf("sentry.missingScopes", &[("label", &conn.label)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    /// The resolved connections decide the tab, not the config file. A row with
    /// a label but no stored token is one the user is still filling in, and
    /// `connections()` is what already knows that - so the answer has to keep
    /// coming from there.
    #[test]
    fn sentry_is_configured_follows_the_connections_not_the_config() {
        let connector = SentryConnector::new();
        let mut cfg = AppConfig::default();
        assert_eq!(
            connector.is_configured(&cfg),
            !config::connections().is_empty(),
            "the answer stopped coming from the guard `fetch` uses",
        );

        cfg.sentry
            .accounts
            .push(crate::engine::config::SentryAccount {
                label: "fastdash-test-account-with-no-token".into(),
                base_url: String::new(),
                organizations: vec!["acme".into()],
            });
        assert_eq!(
            connector.is_configured(&cfg),
            !config::connections().is_empty(),
            "a tokenless account row moved the answer",
        );
    }

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn conn(token: &str) -> SentryConnection {
        SentryConnection {
            label: "work".into(),
            token: token.into(),
            base_url: config::DEFAULT_BASE_URL.into(),
            organizations: vec!["acme".into()],
        }
    }

    /// The window has to cover both end days in full in IST and arrive at
    /// Sentry as UTC - an off-by-one here silently drops the first 5.5 hours of
    /// the range, which is most of a working morning.
    #[test]
    fn the_window_covers_both_ist_end_days_in_utc() {
        let window = utc_window(&DateRange {
            start: day(2026, 8, 3),
            end: day(2026, 8, 3),
        });
        assert_eq!(fmt(window.start), "2026-08-02T18:30:00");
        assert_eq!(fmt(window.end), "2026-08-03T18:29:59");

        let week = utc_window(&DateRange {
            start: day(2026, 7, 28),
            end: day(2026, 8, 3),
        });
        assert_eq!(fmt(week.start), "2026-07-27T18:30:00");
        assert_eq!(fmt(week.end), "2026-08-03T18:29:59");
    }

    /// A dead token is `NeedsAuth` (go reconnect) and a scope problem is
    /// `Misconfigured` (go change a setting); a transient failure must be
    /// neither, or the app nags about a blip.
    #[test]
    fn errors_map_to_the_health_the_user_can_act_on() {
        let c = conn("sntryu_abc");
        assert!(matches!(
            map_error(&c, SentryError::Unauthorized("expired".into())),
            ConnectorError::Auth(_)
        ));
        assert!(matches!(
            map_error(&c, SentryError::Forbidden("no scope".into())),
            ConnectorError::Misconfigured(_)
        ));
        assert!(matches!(
            map_error(
                &c,
                SentryError::RateLimited {
                    retry_after_secs: None
                }
            ),
            ConnectorError::RateLimited
        ));
        assert!(matches!(
            map_error(
                &c,
                SentryError::Status {
                    code: 502,
                    message: "bad gateway".into()
                }
            ),
            ConnectorError::Other(_)
        ));
    }

    /// A CI token has no `event:read` to grant, so telling its owner to tick a
    /// scope box sends them somewhere that does not exist.
    #[test]
    fn an_org_ci_token_gets_its_own_explanation() {
        let ci = forbidden_message(&conn("sntrys_abc123"));
        assert!(ci.contains("organization auth token"), "{ci}");

        let user = forbidden_message(&conn("sntryu_abc123"));
        assert!(!user.contains("organization auth token"), "{user}");
        assert!(user.contains("event:read"), "{user}");
    }

    /// `i18n::t` returns the key itself when a string is missing, so a typo in
    /// a locale key ships silently as `sentry.needsAuth` in the user's banner.
    #[test]
    fn status_copy_resolves() {
        for key in ["sentry.needsAuth", "sentry.orgsFailed", "sentry.noOrgs"] {
            assert_ne!(i18n::t(key), key, "missing catalog entry: {key}");
        }
        for (key, arg) in [
            ("sentry.tokenRejected", ("label", "work")),
            ("sentry.missingScopes", ("label", "work")),
            ("sentry.orgTokenKind", ("label", "work")),
        ] {
            let msg = i18n::tf(key, &[arg]);
            assert_ne!(msg, key, "missing catalog entry: {key}");
            assert!(!msg.contains("{label}"), "unsubstituted: {msg}");
        }
    }
}

#[cfg(test)]
mod live_test {
    use super::*;

    /// Live smoke test against the real Sentry API. Ignored by default; run with:
    ///   SENTRY_AUTH_TOKEN=<token> cargo test --lib sentry::live_test -- --ignored --nocapture
    /// Set FASTDASH_SENTRY_ORGS to pin the organizations (otherwise every
    /// organization the token can see is reported) and FASTDASH_SENTRY_URL for
    /// a region host or a self-hosted install.
    #[ignore = "hits the live Sentry API; run with --ignored and SENTRY_AUTH_TOKEN set"]
    #[tokio::test]
    async fn live_fetch_smoke() {
        let connections = config::connections();
        assert!(
            !connections.is_empty(),
            "set SENTRY_AUTH_TOKEN for the live test"
        );
        for c in &connections {
            eprintln!(
                "account: {} url: {} orgs: {:?}",
                c.label, c.base_url, c.organizations
            );
        }

        let ctx = FetchCtx {
            timezone: "Asia/Kolkata".into(),
            range: DateRange::today(),
        };
        let window = utc_window(&ctx.range);
        eprintln!("window: {} .. {} (UTC)", fmt(window.start), fmt(window.end));

        match SentryConnector::new().fetch(&ctx).await {
            Ok(snapshot) => {
                eprintln!("status: {:?}", snapshot.status);
                eprintln!("panels: {}", snapshot.panels.len());
                eprintln!(
                    "{}",
                    serde_json::to_string_pretty(&snapshot.panels).unwrap()
                );
            }
            Err(e) => panic!("fetch failed: {e}"),
        }
    }

    /// Which organizations and projects the token can actually see, and how
    /// many unresolved issues each organization reports over the last 30 days.
    /// Answers "why is my dashboard empty" without reading panel JSON. Run with:
    ///   SENTRY_AUTH_TOKEN=<token> cargo test --lib sentry::live_test::scope_diag -- --ignored --nocapture
    #[ignore = "hits the live Sentry API; run with --ignored"]
    #[tokio::test]
    async fn scope_diag() {
        let connections = config::connections();
        assert!(
            !connections.is_empty(),
            "set SENTRY_AUTH_TOKEN for the live test"
        );

        for conn in &connections {
            let client = SentryClient::new(&conn.base_url, &conn.token).expect("client");
            match client.organizations().await {
                Ok(orgs) => eprintln!(
                    "{}: token sees organizations {:?}",
                    conn.label,
                    orgs.iter().map(|o| &o.slug).collect::<Vec<_>>()
                ),
                Err(e) => eprintln!("{}: organization list failed: {e}", conn.label),
            }

            let month = DateRange {
                start: range::today_ist() - chrono::Duration::days(29),
                end: range::today_ist(),
            };
            let window = utc_window(&month);
            for org in resolve_orgs(&client, conn).await.unwrap_or_default() {
                match client
                    .issues(&org, QUERY, &fmt(window.start), &fmt(window.end))
                    .await
                {
                    Ok(page) => eprintln!(
                        "  {org}: {} unresolved issues in the last 30 days (truncated: {})",
                        page.issues.len(),
                        page.truncated
                    ),
                    Err(e) => eprintln!("  {org}: {e}"),
                }
            }
        }
    }
}
