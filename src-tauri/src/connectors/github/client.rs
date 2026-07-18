//! Thin GitHub HTTP client: REST Search API (paginated) + a batched GraphQL
//! enrichment call. Holds no aggregation logic.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::Deserialize;

const SEARCH_URL: &str = "https://api.github.com/search/issues";
const GRAPHQL_URL: &str = "https://api.github.com/graphql";
/// Search returns at most 1000 results (10 pages of 100); cap accordingly.
const MAX_PAGES: u32 = 10;
const PER_PAGE: u32 = 100;
/// PRs per GraphQL request. Each PR is a small `repository { pullRequest }`
/// sub-tree; 50 keeps the query well within GitHub's node limits.
const GRAPHQL_CHUNK: usize = 50;

#[derive(Debug, thiserror::Error)]
pub enum GithubError {
    #[error("http error: {0}")]
    Http(String),
    #[error("github returned status {0}")]
    Status(u16),
    #[error("rate limited")]
    RateLimited { retry_after_secs: Option<u64> },
    #[error("parse error: {0}")]
    Parse(String),
    #[error("invalid token header: {0}")]
    Header(String),
    #[error("graphql error: {0}")]
    GraphQl(String),
}

/// A pull request reference (owner/repo/number) drawn from a search result.
#[derive(Debug, Clone)]
pub struct PrRef {
    pub owner: String,
    pub repo: String,
    pub number: u64,
}

/// A single search result item, normalized from the REST Search API.
#[derive(Debug, Clone)]
pub struct SearchItem {
    pub number: u64,
    pub title: String,
    pub html_url: String,
    pub author: Option<String>,
    pub owner: String,
    pub repo: String,
    pub created_at: Option<DateTime<Utc>>,
    pub closed_at: Option<DateTime<Utc>>,
    pub merged_at: Option<DateTime<Utc>>,
}

impl SearchItem {
    pub fn pr_ref(&self) -> PrRef {
        PrRef {
            owner: self.owner.clone(),
            repo: self.repo.clone(),
            number: self.number,
        }
    }

    pub fn key(&self) -> (String, String, u64) {
        (self.owner.clone(), self.repo.clone(), self.number)
    }
}

/// GraphQL-enriched view of a merged-today PR.
#[derive(Debug, Clone)]
pub struct EnrichedPr {
    pub name_with_owner: String,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub author: Option<String>,
    /// GitHub PR state (`OPEN` / `CLOSED` / `MERGED`).
    pub state: String,
    pub additions: u64,
    pub deletions: u64,
    pub merged_at: Option<DateTime<Utc>>,
}

pub struct GithubClient {
    http: reqwest::Client,
}

impl GithubClient {
    pub fn new(token: &str) -> Result<Self, GithubError> {
        use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, USER_AGENT};

        let mut headers = HeaderMap::new();
        let mut auth = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|e| GithubError::Header(e.to_string()))?;
        auth.set_sensitive(true);
        headers.insert(AUTHORIZATION, auth);
        headers.insert(ACCEPT, HeaderValue::from_static("application/vnd.github+json"));
        headers.insert(USER_AGENT, HeaderValue::from_static("fastdash"));
        headers.insert(
            "X-GitHub-Api-Version",
            HeaderValue::from_static("2022-11-28"),
        );

        let http = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .map_err(|e| GithubError::Http(e.to_string()))?;

        Ok(Self { http })
    }

    /// Run a Search API query, following pagination. `query` is the raw `q`
    /// value, e.g. `org:z-roworld type:pr created:<bounds>`.
    pub async fn search_issues(&self, query: &str) -> Result<Vec<SearchItem>, GithubError> {
        let mut items = Vec::new();
        let mut page = 1u32;

        loop {
            let resp = self
                .http
                .get(SEARCH_URL)
                .query(&[
                    ("q", query),
                    ("per_page", &PER_PAGE.to_string()),
                    ("page", &page.to_string()),
                ])
                .send()
                .await
                .map_err(|e| GithubError::Http(e.to_string()))?;

            let status = resp.status();
            let remaining = header_u64(resp.headers(), "x-ratelimit-remaining");

            // Primary/secondary rate limits surface as 403/429.
            if status.as_u16() == 403 || status.as_u16() == 429 {
                if remaining == Some(0) || status.as_u16() == 429 {
                    return Err(GithubError::RateLimited {
                        retry_after_secs: retry_after(resp.headers()),
                    });
                }
                return Err(GithubError::Status(status.as_u16()));
            }
            if !status.is_success() {
                return Err(GithubError::Status(status.as_u16()));
            }

            let body: SearchResponse = resp
                .json()
                .await
                .map_err(|e| GithubError::Parse(e.to_string()))?;

            let count = body.items.len();
            for raw in body.items {
                if let Some(item) = raw.normalize() {
                    items.push(item);
                }
            }

            // Stop when the page was not full (last page) or the cap is hit.
            if count < PER_PAGE as usize || page >= MAX_PAGES {
                break;
            }
            page += 1;

            // Be polite when the Search budget (30/min) runs low.
            if matches!(remaining, Some(r) if r <= 1) {
                return Err(GithubError::RateLimited {
                    retry_after_secs: Some(60),
                });
            }
        }

        Ok(items)
    }

    /// Enrich a set of PRs (by owner/repo/number) with additions, deletions,
    /// author, state, title, `nameWithOwner`, and url in batched GraphQL calls.
    pub async fn enrich_prs(&self, prs: &[PrRef]) -> Result<Vec<EnrichedPr>, GithubError> {
        let mut out = Vec::with_capacity(prs.len());

        for chunk in prs.chunks(GRAPHQL_CHUNK) {
            let query = build_graphql_query(chunk);
            let resp = self
                .http
                .post(GRAPHQL_URL)
                .json(&serde_json::json!({ "query": query }))
                .send()
                .await
                .map_err(|e| GithubError::Http(e.to_string()))?;

            let status = resp.status();
            if status.as_u16() == 403 || status.as_u16() == 429 {
                return Err(GithubError::RateLimited {
                    retry_after_secs: retry_after(resp.headers()),
                });
            }
            if !status.is_success() {
                return Err(GithubError::Status(status.as_u16()));
            }

            let body: GraphQlResponse = resp
                .json()
                .await
                .map_err(|e| GithubError::Parse(e.to_string()))?;

            if let Some(errors) = body.errors {
                // Node-not-found style errors are non-fatal (repo/PR moved);
                // only bail if no data came back at all.
                if body.data.is_none() {
                    return Err(GithubError::GraphQl(errors.to_string()));
                }
            }

            let Some(data) = body.data else { continue };
            for i in 0..chunk.len() {
                let alias = format!("r{i}");
                let Some(repo) = data.get(&alias) else {
                    continue;
                };
                if repo.is_null() {
                    continue;
                }
                if let Some(enriched) = parse_enriched(repo) {
                    out.push(enriched);
                }
            }
        }

        Ok(out)
    }
}

/// Build a batched GraphQL query aliasing each PR as `r0`, `r1`, ....
fn build_graphql_query(chunk: &[PrRef]) -> String {
    let mut q = String::from("query {\n");
    for (i, pr) in chunk.iter().enumerate() {
        // Repo owner/name characters are constrained (alphanumerics, `-`, `_`,
        // `.`); still, escape defensively for the embedded string literals.
        q.push_str(&format!(
            "  r{i}: repository(owner: \"{owner}\", name: \"{repo}\") {{ \
                nameWithOwner \
                pullRequest(number: {number}) {{ \
                    number title url additions deletions state mergedAt \
                    author {{ login }} \
                }} \
            }}\n",
            i = i,
            owner = escape_gql(&pr.owner),
            repo = escape_gql(&pr.repo),
            number = pr.number,
        ));
    }
    q.push('}');
    q
}

fn escape_gql(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn parse_enriched(repo: &serde_json::Value) -> Option<EnrichedPr> {
    let name_with_owner = repo.get("nameWithOwner")?.as_str()?.to_string();
    let pr = repo.get("pullRequest")?;
    if pr.is_null() {
        return None;
    }
    Some(EnrichedPr {
        name_with_owner,
        number: pr.get("number").and_then(|v| v.as_u64()).unwrap_or(0),
        title: pr
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        url: pr
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        author: pr
            .get("author")
            .and_then(|a| a.get("login"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        state: pr
            .get("state")
            .and_then(|v| v.as_str())
            .unwrap_or("UNKNOWN")
            .to_string(),
        additions: pr.get("additions").and_then(|v| v.as_u64()).unwrap_or(0),
        deletions: pr.get("deletions").and_then(|v| v.as_u64()).unwrap_or(0),
        merged_at: pr
            .get("mergedAt")
            .and_then(|v| v.as_str())
            .and_then(parse_ts),
    })
}

fn header_u64(headers: &reqwest::header::HeaderMap, name: &str) -> Option<u64> {
    headers.get(name)?.to_str().ok()?.trim().parse().ok()
}

/// Seconds until the rate-limit window resets, from `Retry-After` (relative) or
/// `X-RateLimit-Reset` (absolute epoch).
fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    if let Some(secs) = header_u64(headers, "retry-after") {
        return Some(secs);
    }
    let reset = header_u64(headers, "x-ratelimit-reset")?;
    let now = Utc::now().timestamp().max(0) as u64;
    Some(reset.saturating_sub(now).max(1))
}

fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

// --- REST Search wire types ---

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    items: Vec<RawSearchItem>,
}

#[derive(Debug, Deserialize)]
struct RawSearchItem {
    number: u64,
    title: String,
    html_url: String,
    created_at: Option<String>,
    closed_at: Option<String>,
    repository_url: String,
    user: Option<RawUser>,
    pull_request: Option<RawPullRequest>,
}

#[derive(Debug, Deserialize)]
struct RawUser {
    login: String,
}

#[derive(Debug, Deserialize)]
struct RawPullRequest {
    merged_at: Option<String>,
}

impl RawSearchItem {
    fn normalize(self) -> Option<SearchItem> {
        let (owner, repo) = owner_repo_from_url(&self.repository_url)?;
        Some(SearchItem {
            number: self.number,
            title: self.title,
            html_url: self.html_url,
            author: self.user.map(|u| u.login),
            owner,
            repo,
            created_at: self.created_at.as_deref().and_then(parse_ts),
            closed_at: self.closed_at.as_deref().and_then(parse_ts),
            merged_at: self
                .pull_request
                .and_then(|p| p.merged_at)
                .as_deref()
                .and_then(parse_ts),
        })
    }
}

/// `https://api.github.com/repos/OWNER/REPO` -> `(OWNER, REPO)`.
fn owner_repo_from_url(url: &str) -> Option<(String, String)> {
    let tail = url.rsplit("/repos/").next()?;
    let mut parts = tail.splitn(2, '/');
    let owner = parts.next()?.to_string();
    let repo = parts.next()?.to_string();
    if owner.is_empty() || repo.is_empty() {
        None
    } else {
        Some((owner, repo))
    }
}

// --- GraphQL wire types ---

#[derive(Debug, Deserialize)]
struct GraphQlResponse {
    data: Option<HashMap<String, serde_json::Value>>,
    errors: Option<serde_json::Value>,
}
