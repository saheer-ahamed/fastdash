//! Thin Slack Web API client for the connector.
//!
//! Only two endpoints are needed for v1:
//!   - `auth.test`       resolve the current user id + workspace name/url
//!   - `search.messages` find messages that mention the current user
//!
//! Both require a **user token** (`xoxp`) with `search:read`; bot tokens
//! (`xoxb`) are rejected by `search.messages` with `not_allowed_token_type`.
//!
//! Every Slack response is an envelope `{ "ok": bool, "error"?: string, ... }`.
//! We surface `ok:false` as a typed [`SlackError`] so the connector can decide
//! whether it means "needs auth", "rate limited", or a hard error.

use serde::Deserialize;
use std::time::Duration;

const AUTH_TEST_URL: &str = "https://slack.com/api/auth.test";
const SEARCH_MESSAGES_URL: &str = "https://slack.com/api/search.messages";
const USER_AGENT: &str = "fastdash/0.1";

/// How many matches to request per `search.messages` page (Slack max is 100).
const SEARCH_PAGE_SIZE: u32 = 100;

#[derive(Debug, thiserror::Error)]
pub enum SlackError {
    #[error("slack request failed: {0}")]
    Http(String),
    #[error("slack api returned http status {0}")]
    Status(u16),
    #[error("could not parse slack response: {0}")]
    Parse(String),
    /// The `ok:false` envelope; carries Slack's machine-readable `error` string
    /// (e.g. `invalid_auth`, `missing_scope`, `ratelimited`).
    #[error("slack api error: {0}")]
    Api(String),
}

impl SlackError {
    /// True when the error means the token is missing/invalid/insufficient and
    /// the user must re-authenticate (as opposed to a transient failure).
    pub fn is_auth_problem(&self) -> bool {
        matches!(
            self.api_code(),
            Some(
                "not_authed"
                    | "invalid_auth"
                    | "token_revoked"
                    | "token_expired"
                    | "account_inactive"
                    | "no_permission"
                    | "missing_scope"
                    | "not_allowed_token_type"
            )
        )
    }

    /// True when Slack asked us to back off.
    pub fn is_rate_limited(&self) -> bool {
        matches!(self, SlackError::Status(429)) || self.api_code() == Some("ratelimited")
    }

    /// The Slack `error` code, when this is an API envelope error.
    pub fn api_code(&self) -> Option<&str> {
        match self {
            SlackError::Api(code) => Some(code.as_str()),
            _ => None,
        }
    }
}

/// Authenticated Slack Web API client.
pub struct SlackClient {
    http: reqwest::Client,
    token: String,
}

impl SlackClient {
    pub fn new(token: String) -> Result<Self, SlackError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .user_agent(USER_AGENT)
            .build()
            .map_err(|e| SlackError::Http(e.to_string()))?;
        Ok(SlackClient { http, token })
    }

    /// `auth.test` - resolves the identity behind the token.
    pub async fn auth_test(&self) -> Result<AuthTest, SlackError> {
        let resp = self
            .http
            .post(AUTH_TEST_URL)
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|e| SlackError::Http(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(SlackError::Status(resp.status().as_u16()));
        }

        let parsed: AuthTest = resp
            .json()
            .await
            .map_err(|e| SlackError::Parse(e.to_string()))?;

        if !parsed.ok {
            return Err(SlackError::Api(
                parsed.error.unwrap_or_else(|| "unknown_error".into()),
            ));
        }
        Ok(parsed)
    }

    /// One page of `search.messages`. `page` is 1-based.
    ///
    /// `query` is the raw Slack search query (e.g. `<@U123> after:2026-07-17`).
    /// Results are sorted by timestamp ascending so paging is stable.
    pub async fn search_messages(
        &self,
        query: &str,
        page: u32,
    ) -> Result<SearchMessages, SlackError> {
        let count = SEARCH_PAGE_SIZE.to_string();
        let page = page.to_string();
        let resp = self
            .http
            .get(SEARCH_MESSAGES_URL)
            .bearer_auth(&self.token)
            .query(&[
                ("query", query),
                ("sort", "timestamp"),
                ("sort_dir", "asc"),
                ("count", count.as_str()),
                ("page", page.as_str()),
            ])
            .send()
            .await
            .map_err(|e| SlackError::Http(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(SlackError::Status(resp.status().as_u16()));
        }

        let parsed: SearchResponse = resp
            .json()
            .await
            .map_err(|e| SlackError::Parse(e.to_string()))?;

        if !parsed.ok {
            return Err(SlackError::Api(
                parsed.error.unwrap_or_else(|| "unknown_error".into()),
            ));
        }
        Ok(parsed.messages.unwrap_or_default())
    }
}

// --- wire types -------------------------------------------------------------

/// Response of `auth.test` (only the fields we use).
#[derive(Debug, Clone, Deserialize)]
pub struct AuthTest {
    pub ok: bool,
    pub error: Option<String>,
    /// Workspace base url, e.g. `https://acme.slack.com/`.
    pub url: Option<String>,
    /// Human workspace name, e.g. `Acme`.
    pub team: Option<String>,
    /// Display name of the authed user.
    pub user: Option<String>,
    /// `Uxxxx` id used to build the `<@Uxxxx>` mention query.
    pub user_id: Option<String>,
    pub team_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    ok: bool,
    error: Option<String>,
    messages: Option<SearchMessages>,
}

/// The `messages` object of a `search.messages` response.
#[derive(Debug, Default, Deserialize)]
pub struct SearchMessages {
    #[serde(default)]
    pub matches: Vec<Match>,
    /// Legacy paging block (`page` / `pages`); present on `search.messages`.
    pub paging: Option<Paging>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Paging {
    pub page: Option<u32>,
    pub pages: Option<u32>,
}

/// One matched message.
#[derive(Debug, Clone, Deserialize)]
pub struct Match {
    /// Slack timestamp, e.g. `"1610000000.000200"` (epoch seconds.microseconds).
    pub ts: Option<String>,
    pub text: Option<String>,
    /// Deep link to the message in Slack.
    pub permalink: Option<String>,
    pub channel: Option<Channel>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Channel {
    pub id: Option<String>,
    pub name: Option<String>,
}
