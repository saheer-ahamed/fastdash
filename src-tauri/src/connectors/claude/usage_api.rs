//! Official Claude `/usage` pull.
//!
//! Confirmed by spike (HTTP 200):
//!   GET https://api.anthropic.com/api/oauth/usage
//!   Authorization: Bearer <claudeAiOauth.accessToken from ~/.claude/.credentials.json>
//!   anthropic-beta: oauth-2025-04-20
//!
//! Response carries `five_hour` and `seven_day` buckets ({ utilization, resets_at })
//! and a richer `limits[]` array with per-kind entries (session / weekly_all /
//! weekly_scoped-per-model). Utilization/percent are 0..=100; `resets_at` is
//! RFC3339. The subscription plan returns percentages and reset times only -
//! absolute token counts come from local transcript aggregation, so the two
//! sources are combined by the connector.
//!
//! If the endpoint is unavailable (offline, 401, shape change), the caller falls
//! back to limits estimated from local history so the meters never go blank.
//!
//! TODO(feat/claude): refresh the OAuth token via `claudeAiOauth.refreshToken`
//! when `expiresAt` has passed (Claude Code also rewrites the file on refresh).

use chrono::{DateTime, Utc};
use serde::Deserialize;

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const OAUTH_BETA: &str = "oauth-2025-04-20";

#[derive(Debug, thiserror::Error)]
pub enum UsageError {
    #[error("home directory not found")]
    NoHome,
    #[error("credentials not found or unreadable: {0}")]
    Credentials(String),
    #[error("no OAuth access token in credentials")]
    NoToken,
    #[error("usage request failed: {0}")]
    Http(String),
    #[error("usage endpoint returned status {0}")]
    Status(u16),
    #[error("could not parse usage response: {0}")]
    Parse(String),
}

/// One rate-limit window in normalized form.
#[derive(Debug, Clone)]
pub struct UsageWindow {
    /// 0..=100.
    pub percent: f64,
    pub resets_at: Option<DateTime<Utc>>,
}

/// A per-model (or otherwise scoped) weekly limit.
#[derive(Debug, Clone)]
pub struct ScopedLimit {
    pub label: String,
    pub percent: f64,
    pub resets_at: Option<DateTime<Utc>>,
}

/// The official numbers, normalized for the UI.
#[derive(Debug, Clone, Default)]
pub struct OfficialUsage {
    pub five_hour: Option<UsageWindow>,
    pub weekly: Option<UsageWindow>,
    pub scoped: Vec<ScopedLimit>,
}

/// Read the subscription OAuth access token from `~/.claude/.credentials.json`.
pub fn read_oauth_token() -> Result<String, UsageError> {
    let base = directories::BaseDirs::new().ok_or(UsageError::NoHome)?;
    let path = base.home_dir().join(".claude").join(".credentials.json");
    let raw = std::fs::read_to_string(&path).map_err(|e| UsageError::Credentials(e.to_string()))?;
    let v: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| UsageError::Credentials(e.to_string()))?;
    v["claudeAiOauth"]["accessToken"]
        .as_str()
        .map(str::to_owned)
        .ok_or(UsageError::NoToken)
}

/// Fetch and normalize the official usage numbers.
pub async fn fetch_official_usage(token: &str) -> Result<OfficialUsage, UsageError> {
    let resp = reqwest::Client::new()
        .get(USAGE_URL)
        .bearer_auth(token)
        .header("anthropic-beta", OAUTH_BETA)
        .header("anthropic-version", "2023-06-01")
        .header("User-Agent", "fastdash/0.1")
        .send()
        .await
        .map_err(|e| UsageError::Http(e.to_string()))?;

    if !resp.status().is_success() {
        return Err(UsageError::Status(resp.status().as_u16()));
    }

    let parsed: UsageResponse = resp.json().await.map_err(|e| UsageError::Parse(e.to_string()))?;
    Ok(parsed.normalize())
}

// --- wire types matching /api/oauth/usage ---

#[derive(Debug, Deserialize)]
struct UsageResponse {
    five_hour: Option<Bucket>,
    seven_day: Option<Bucket>,
    #[serde(default)]
    limits: Vec<LimitEntry>,
}

#[derive(Debug, Deserialize)]
struct Bucket {
    utilization: Option<f64>,
    resets_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LimitEntry {
    kind: Option<String>,
    percent: Option<f64>,
    resets_at: Option<String>,
    scope: Option<Scope>,
}

#[derive(Debug, Deserialize)]
struct Scope {
    model: Option<ScopeModel>,
}

#[derive(Debug, Deserialize)]
struct ScopeModel {
    display_name: Option<String>,
}

fn parse_ts(s: &Option<String>) -> Option<DateTime<Utc>> {
    s.as_deref()
        .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
        .map(|dt| dt.with_timezone(&Utc))
}

impl UsageResponse {
    /// Prefer the structured `limits[]` array; fall back to the flat buckets.
    fn normalize(self) -> OfficialUsage {
        let mut out = OfficialUsage::default();

        for l in &self.limits {
            let window = UsageWindow {
                percent: l.percent.unwrap_or(0.0),
                resets_at: parse_ts(&l.resets_at),
            };
            match l.kind.as_deref() {
                Some("session") => out.five_hour = Some(window),
                Some("weekly_all") => out.weekly = Some(window),
                Some("weekly_scoped") => {
                    let label = l
                        .scope
                        .as_ref()
                        .and_then(|s| s.model.as_ref())
                        .and_then(|m| m.display_name.clone())
                        .unwrap_or_else(|| "scoped".to_string());
                    out.scoped.push(ScopedLimit {
                        label,
                        percent: window.percent,
                        resets_at: window.resets_at,
                    });
                }
                _ => {}
            }
        }

        if out.five_hour.is_none() {
            if let Some(b) = &self.five_hour {
                out.five_hour = Some(UsageWindow {
                    percent: b.utilization.unwrap_or(0.0),
                    resets_at: parse_ts(&b.resets_at),
                });
            }
        }
        if out.weekly.is_none() {
            if let Some(b) = &self.seven_day {
                out.weekly = Some(UsageWindow {
                    percent: b.utilization.unwrap_or(0.0),
                    resets_at: parse_ts(&b.resets_at),
                });
            }
        }

        out
    }
}
