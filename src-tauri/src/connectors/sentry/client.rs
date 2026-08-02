//! Thin Sentry HTTP client: the organization list and the paginated
//! organization-issues endpoint. Holds no aggregation logic.
//!
//! Everything talks to `{base_url}/api/0`, so the same client serves SaaS
//! (`https://sentry.io`, or a region host like `https://de.sentry.io`) and a
//! self-hosted install.

use chrono::{DateTime, Utc};
use serde::Deserialize;

/// Issues per page. 100 is the endpoint's maximum.
const PER_PAGE: u32 = 100;
/// Pages to follow before giving up. 5 x 100 issues is far more than a
/// dashboard can show; past that the totals are reported as partial rather
/// than paginating until the rate limiter objects.
const MAX_PAGES: u32 = 5;
/// Culprits can be a whole stack frame. Real ones (`app/routes/index`,
/// `sentry.tasks.process`) sit well under this; the cap is for the pathological
/// ones, which would otherwise push the table into a horizontal scroll on their
/// own. The linked issue has the full text either way.
const MAX_CULPRIT: usize = 40;

#[derive(Debug, thiserror::Error)]
pub enum SentryError {
    #[error("http error: {0}")]
    Http(String),
    #[error("sentry returned status {code}: {message}")]
    Status { code: u16, message: String },
    /// The token is missing, expired, or revoked.
    #[error("unauthorized: {0}")]
    Unauthorized(String),
    /// The token is valid but lacks the scope, or the org forbids it.
    #[error("forbidden: {0}")]
    Forbidden(String),
    /// The organization slug does not resolve for this token.
    #[error("not found")]
    NotFound,
    #[error("rate limited")]
    RateLimited { retry_after_secs: Option<u64> },
    #[error("parse error: {0}")]
    Parse(String),
    #[error("invalid token header: {0}")]
    Header(String),
    #[error("invalid Sentry URL: {0}")]
    BadUrl(String),
}

/// An organization the token can see.
#[derive(Debug, Clone)]
pub struct Organization {
    pub slug: String,
}

/// One issue from the organization issue stream, normalized.
#[derive(Debug, Clone)]
pub struct Issue {
    pub title: String,
    /// Where it happened (Sentry's `culprit`), already shortened for a cell.
    pub culprit: Option<String>,
    /// Link to the issue on Sentry.
    pub permalink: Option<String>,
    /// `error`, `warning`, `fatal`, ... Absent on some issue types.
    pub level: Option<String>,
    /// Owning project slug. The issue stream always carries one; `None` is the
    /// defensive case, and it stays out of the per-project rollups rather than
    /// becoming a phantom project of its own.
    pub project: Option<String>,
    /// Events in the requested window.
    pub events: u64,
    /// Distinct users affected in the requested window.
    pub users: u64,
    pub first_seen: Option<DateTime<Utc>>,
    pub last_seen: Option<DateTime<Utc>>,
}

/// One page-following pass over the issue stream.
#[derive(Debug, Clone)]
pub struct IssuePage {
    pub issues: Vec<Issue>,
    /// Sentry still had more pages when [`MAX_PAGES`] ran out, so the totals
    /// below it are a floor rather than the whole picture.
    pub truncated: bool,
}

pub struct SentryClient {
    http: reqwest::Client,
    /// `{base_url}/api/0`, without a trailing slash.
    api: String,
}

impl SentryClient {
    pub fn new(base_url: &str, token: &str) -> Result<Self, SentryError> {
        use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, USER_AGENT};

        let mut headers = HeaderMap::new();
        let mut auth = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|e| SentryError::Header(e.to_string()))?;
        auth.set_sensitive(true);
        headers.insert(AUTHORIZATION, auth);
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(USER_AGENT, HeaderValue::from_static("fastdash"));

        let http = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .map_err(|e| SentryError::Http(e.to_string()))?;

        Ok(SentryClient {
            http,
            api: format!("{}/api/0", normalize_base_url(base_url)?),
        })
    }

    /// Organizations this token can see. Used when the account lists none, so
    /// a fresh connection needs a token and nothing else.
    pub async fn organizations(&self) -> Result<Vec<Organization>, SentryError> {
        let resp = self
            .http
            .get(format!("{}/organizations/", self.api))
            .send()
            .await
            .map_err(|e| SentryError::Http(e.to_string()))?;

        let resp = check(resp).await?;
        let raw: Vec<RawOrganization> = resp
            .json()
            .await
            .map_err(|e| SentryError::Parse(e.to_string()))?;

        Ok(raw
            .into_iter()
            .filter(|o| !o.slug.is_empty())
            .map(|o| Organization { slug: o.slug })
            .collect())
    }

    /// The organization's issue stream over `[start, end]`, following the
    /// cursor pagination in the `Link` header.
    ///
    /// `start` / `end` are naive UTC timestamps paired with `utc=true`, so the
    /// window means the same thing whatever timezone the org is configured in.
    /// `project=-1` is Sentry's "every project this token can read": narrowing
    /// takes numeric project ids, which would cost a second round trip to
    /// resolve, and the per-project breakdown is derived from the results
    /// instead.
    pub async fn issues(
        &self,
        org: &str,
        query: &str,
        start: &str,
        end: &str,
    ) -> Result<IssuePage, SentryError> {
        let url = format!("{}/organizations/{org}/issues/", self.api);
        let mut issues = Vec::new();
        let mut cursor: Option<String> = None;
        let mut truncated = false;

        for page in 0..MAX_PAGES {
            let mut params: Vec<(&str, String)> = vec![
                ("query", query.to_string()),
                ("start", start.to_string()),
                ("end", end.to_string()),
                ("utc", "true".to_string()),
                // Sentry defaults to a 14-day statsPeriod when it is absent,
                // and refuses start/end alongside a non-empty one. An empty
                // value is falsy on their side, which is what hands the window
                // to start/end.
                ("statsPeriod", String::new()),
                ("project", "-1".to_string()),
                ("sort", "freq".to_string()),
                ("limit", PER_PAGE.to_string()),
            ];
            if let Some(c) = &cursor {
                params.push(("cursor", c.clone()));
            }

            let resp = self
                .http
                .get(&url)
                .query(&params)
                .send()
                .await
                .map_err(|e| SentryError::Http(e.to_string()))?;

            let resp = check(resp).await?;
            let next = next_cursor(resp.headers());
            let raw: Vec<RawIssue> = resp
                .json()
                .await
                .map_err(|e| SentryError::Parse(e.to_string()))?;

            issues.extend(raw.into_iter().map(RawIssue::normalize));

            match next {
                Some(c) if page + 1 < MAX_PAGES => cursor = Some(c),
                Some(_) => {
                    truncated = true;
                    break;
                }
                None => break,
            }
        }

        Ok(IssuePage { issues, truncated })
    }
}

/// Map a response's status onto [`SentryError`], or hand back the response.
///
/// The four that get their own variant are the ones the UI answers differently:
/// a dead token, a token missing a scope, a wrong org slug, and a rate limit.
/// Everything else stays a generic `Status`, which reads as "transient, we'll
/// keep trying".
async fn check(resp: reqwest::Response) -> Result<reqwest::Response, SentryError> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    if status.as_u16() == 429 {
        return Err(SentryError::RateLimited {
            retry_after_secs: retry_after(resp.headers()),
        });
    }
    let code = status.as_u16();
    let message = detail(resp).await;
    match code {
        401 => Err(SentryError::Unauthorized(message)),
        403 => Err(SentryError::Forbidden(message)),
        404 => Err(SentryError::NotFound),
        _ => Err(SentryError::Status { code, message }),
    }
}

/// Sentry's error body is `{"detail": "..."}`, sometimes with field errors
/// alongside. Fall back to the raw body so nothing is swallowed.
async fn detail(resp: reqwest::Response) -> String {
    let body = resp.text().await.unwrap_or_default();
    serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| {
            v.get("detail")
                .and_then(|d| d.as_str())
                .map(str::to_string)
                .or_else(|| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
        })
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| {
            if body.trim().is_empty() {
                "no response body".to_string()
            } else {
                body.chars().take(300).collect()
            }
        })
}

/// Seconds until the rate-limit window reopens, from `Retry-After` or Sentry's
/// own `X-Sentry-Rate-Limit-Reset` (an absolute epoch).
fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    let read =
        |name: &str| -> Option<u64> { headers.get(name)?.to_str().ok()?.trim().parse().ok() };
    if let Some(secs) = read("retry-after") {
        return Some(secs);
    }
    let reset = read("x-sentry-rate-limit-reset")?;
    let now = Utc::now().timestamp().max(0) as u64;
    Some(reset.saturating_sub(now).max(1))
}

/// The cursor for the next page of a Sentry `Link` header, or `None` at the
/// end of the stream.
///
/// Sentry always sends both a `previous` and a `next` link and marks the empty
/// one `results="false"`, so the flag - not the link's presence - is what says
/// whether another page exists.
fn next_cursor(headers: &reqwest::header::HeaderMap) -> Option<String> {
    let link = headers.get(reqwest::header::LINK)?.to_str().ok()?;
    parse_next_cursor(link)
}

fn parse_next_cursor(link: &str) -> Option<String> {
    // Cursors are `offset:limit:is_prev` and Sentry percent-encodes the query,
    // so no comma can appear inside one link and this split is unambiguous.
    for part in link.split(',') {
        if attr(part, "rel").as_deref() != Some("next") {
            continue;
        }
        if attr(part, "results").as_deref() != Some("true") {
            return None;
        }
        return attr(part, "cursor");
    }
    None
}

/// The value of a named parameter in one `Link` header entry.
///
/// Matched per `;`-separated attribute rather than by scanning the whole entry:
/// the URL that opens it carries its own `?cursor=...`, and a plain substring
/// search finds that one first - which reads as "no next page" and silently
/// caps every fetch at a single page.
fn attr(part: &str, name: &str) -> Option<String> {
    part.split(';')
        .skip(1)
        .filter_map(|a| a.trim().split_once('='))
        .find(|(key, _)| key.trim() == name)
        .map(|(_, value)| value.trim().trim_matches('"').to_string())
}

/// Canonical origin for the API: scheme-qualified, no trailing slash, and with
/// any `/api/0` the user pasted from their browser stripped back off.
fn normalize_base_url(raw: &str) -> Result<String, SentryError> {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(SentryError::BadUrl(raw.to_string()));
    }
    // A bare host is the common paste; assume https rather than failing.
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    if !with_scheme.starts_with("https://") && !with_scheme.starts_with("http://") {
        return Err(SentryError::BadUrl(raw.to_string()));
    }
    Ok(with_scheme
        .trim_end_matches("/api/0")
        .trim_end_matches('/')
        .to_string())
}

fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// `count` arrives as a string on the issue stream but as a number elsewhere in
/// Sentry's API, so accept either rather than dropping the field.
fn as_count(value: Option<&serde_json::Value>) -> u64 {
    match value {
        Some(serde_json::Value::Number(n)) => n.as_u64().unwrap_or(0),
        Some(serde_json::Value::String(s)) => s.parse().unwrap_or(0),
        _ => 0,
    }
}

fn shorten(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{}\u{2026}", head.trim_end())
}

// --- wire types ---

#[derive(Debug, Deserialize)]
struct RawOrganization {
    #[serde(default)]
    slug: String,
}

#[derive(Debug, Deserialize)]
struct RawIssue {
    #[serde(default)]
    title: String,
    culprit: Option<String>,
    permalink: Option<String>,
    level: Option<String>,
    project: Option<RawProject>,
    count: Option<serde_json::Value>,
    #[serde(rename = "userCount")]
    user_count: Option<u64>,
    #[serde(rename = "firstSeen")]
    first_seen: Option<String>,
    #[serde(rename = "lastSeen")]
    last_seen: Option<String>,
    /// Present when the search narrowed the events counted; then `count` is the
    /// unfiltered window total and this is the one that matches the query.
    filtered: Option<RawFiltered>,
}

#[derive(Debug, Deserialize)]
struct RawFiltered {
    count: Option<serde_json::Value>,
    #[serde(rename = "userCount")]
    user_count: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct RawProject {
    #[serde(default)]
    slug: String,
}

impl RawIssue {
    fn normalize(self) -> Issue {
        let filtered = self.filtered.as_ref();
        Issue {
            title: if self.title.trim().is_empty() {
                "(untitled)".to_string()
            } else {
                self.title
            },
            culprit: self
                .culprit
                .filter(|c| !c.trim().is_empty())
                .map(|c| shorten(&c, MAX_CULPRIT)),
            permalink: self.permalink.filter(|p| p.starts_with("http")),
            level: self.level.filter(|l| !l.trim().is_empty()),
            project: self.project.map(|p| p.slug).filter(|s| !s.is_empty()),
            events: as_count(
                filtered
                    .and_then(|f| f.count.as_ref())
                    .or(self.count.as_ref()),
            ),
            users: filtered
                .and_then(|f| f.user_count)
                .or(self.user_count)
                .unwrap_or(0),
            first_seen: self.first_seen.as_deref().and_then(parse_ts),
            last_seen: self.last_seen.as_deref().and_then(parse_ts),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_urls_are_normalized_to_an_origin() {
        for (input, want) in [
            ("https://sentry.io", "https://sentry.io"),
            ("https://sentry.io/", "https://sentry.io"),
            ("sentry.io", "https://sentry.io"),
            ("https://de.sentry.io///", "https://de.sentry.io"),
            ("http://localhost:9000", "http://localhost:9000"),
            // Pasted straight out of the API docs' browser tab.
            (
                "https://sentry.example.com/api/0/",
                "https://sentry.example.com",
            ),
        ] {
            assert_eq!(normalize_base_url(input).unwrap(), want, "input: {input}");
        }
        assert!(normalize_base_url("   ").is_err());
        assert!(normalize_base_url("ftp://sentry.io").is_err());
    }

    /// Sentry sends both links every time; only `results="true"` means there is
    /// another page, so keying off the link's presence would loop to MAX_PAGES
    /// on every single fetch.
    #[test]
    fn only_a_results_true_next_link_yields_a_cursor() {
        let more = concat!(
            r#"<https://sentry.io/api/0/organizations/acme/issues/?cursor=0%3A0%3A1>; "#,
            r#"rel="previous"; results="false"; cursor="0:0:1", "#,
            r#"<https://sentry.io/api/0/organizations/acme/issues/?cursor=0%3A100%3A0>; "#,
            r#"rel="next"; results="true"; cursor="0:100:0""#,
        );
        assert_eq!(parse_next_cursor(more).as_deref(), Some("0:100:0"));

        let done = concat!(
            r#"<https://sentry.io/api/0/organizations/acme/issues/?cursor=0%3A0%3A1>; "#,
            r#"rel="previous"; results="false"; cursor="0:0:1", "#,
            r#"<https://sentry.io/api/0/organizations/acme/issues/?cursor=0%3A100%3A0>; "#,
            r#"rel="next"; results="false"; cursor="0:100:0""#,
        );
        assert_eq!(parse_next_cursor(done), None);
        assert_eq!(parse_next_cursor(""), None);
    }

    /// `count` is a string on this endpoint and a number on others; reading
    /// only one of the two would silently report every issue as 0 events.
    #[test]
    fn event_counts_parse_from_either_json_type() {
        assert_eq!(as_count(Some(&serde_json::json!("42"))), 42);
        assert_eq!(as_count(Some(&serde_json::json!(42))), 42);
        assert_eq!(as_count(Some(&serde_json::json!(null))), 0);
        assert_eq!(as_count(None), 0);
    }

    /// When Sentry reports a filtered subtotal, that is the one matching the
    /// search - `count` beside it is the unfiltered window total.
    #[test]
    fn filtered_counts_win_over_the_window_total() {
        let raw: RawIssue = serde_json::from_value(serde_json::json!({
            "title": "TypeError: x is not a function",
            "culprit": "app/routes/index",
            "permalink": "https://acme.sentry.io/issues/1/",
            "level": "error",
            "project": { "slug": "frontend" },
            "count": "100",
            "userCount": 20,
            "filtered": { "count": "7", "userCount": 3 },
            "firstSeen": "2026-08-03T04:30:00Z",
            "lastSeen": "2026-08-03T09:12:00Z"
        }))
        .unwrap();

        let issue = raw.normalize();
        assert_eq!(issue.events, 7);
        assert_eq!(issue.users, 3);
        assert_eq!(issue.project.as_deref(), Some("frontend"));
        assert_eq!(issue.level.as_deref(), Some("error"));
        assert!(issue.first_seen.is_some());
    }

    /// A sparse issue must still render: no project, no level, no culprit, and
    /// a plain numeric count are all shapes Sentry actually returns.
    #[test]
    fn sparse_issues_still_normalize() {
        let raw: RawIssue = serde_json::from_value(serde_json::json!({
            "title": "",
            "count": 3
        }))
        .unwrap();

        let issue = raw.normalize();
        assert_eq!(issue.title, "(untitled)");
        assert_eq!(issue.project, None);
        assert_eq!(issue.events, 3);
        assert_eq!(issue.users, 0);
        assert_eq!(issue.culprit, None);
        assert_eq!(issue.permalink, None);
    }

    /// A relative permalink (some self-hosted installs) is dropped rather than
    /// rendered as a link the webview cannot open.
    #[test]
    fn non_http_permalinks_are_dropped() {
        let raw: RawIssue =
            serde_json::from_value(serde_json::json!({ "permalink": "/issues/1/" })).unwrap();
        assert_eq!(raw.normalize().permalink, None);
    }

    #[test]
    fn long_culprits_are_ellipsized_not_cut_mid_char() {
        let long = "a".repeat(200);
        let out = shorten(&long, MAX_CULPRIT);
        assert_eq!(out.chars().count(), MAX_CULPRIT);
        assert!(out.ends_with('\u{2026}'));
        assert_eq!(shorten("short", MAX_CULPRIT), "short");
    }
}
