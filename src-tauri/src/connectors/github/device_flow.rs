//! GitHub OAuth **Device Flow** - the browser login used by desktop/CLI apps
//! (it is exactly how `gh auth login` works).
//!
//! Flow:
//!   1. `start()` asks GitHub for a `device_code` + a short human `user_code`,
//!      and opens the browser to `verification_uri`.
//!   2. The user types the `user_code` and approves the requested scopes.
//!   3. `poll()` long-polls the token endpoint until GitHub returns an
//!      `access_token` (or the code expires / is denied).
//!
//! Device Flow needs only a **public client id** - no client secret, no
//! redirect URL, no local web server - which is why it is the right fit for a
//! secret-less desktop app. Register an OAuth App once, tick *Enable Device
//! Flow*, and put its Client ID in `CLIENT_ID` below (or the
//! `FASTDASH_GITHUB_CLIENT_ID` env var for local dev).

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

const DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
const TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const USER_URL: &str = "https://api.github.com/user";
const GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// Scopes fastdash needs: read org membership and PRs, identify the user, and
/// read private repositories the user can see.
const SCOPES: &str = "repo read:org read:user";

/// Public OAuth App client id, **baked in at build time** from the
/// `FASTDASH_GITHUB_CLIENT_ID` env var set on the build machine. **Not a
/// secret** - it is the app's public identifier and the same for every user;
/// the compiled-in value ships in the binary so end users get it automatically.
///
/// Register an OAuth App at <https://github.com/settings/developers>, enable
/// *Device Flow*, and set `FASTDASH_GITHUB_CLIENT_ID` when building a release
/// (e.g. in CI). A build without it yields `None` and a clear "not configured"
/// message - fine for contributors, who supply their own id if they want to
/// exercise the login locally.
const CLIENT_ID: Option<&str> = option_env!("FASTDASH_GITHUB_CLIENT_ID");

/// Absolute ceiling on the poll loop so a wedged flow can never spin forever;
/// GitHub codes expire well before this (typically 900s).
const MAX_POLL_SECS: u64 = 900;

/// Resolve the OAuth App client id, or `None` if none is configured.
///
/// Order: a **runtime** `FASTDASH_GITHUB_CLIENT_ID` (dev override, no rebuild
/// needed) wins; otherwise the **build-time** value compiled into `CLIENT_ID`
/// (what ships to users) is used.
fn client_id() -> Option<String> {
    if let Ok(v) = std::env::var("FASTDASH_GITHUB_CLIENT_ID") {
        let v = v.trim().to_string();
        if !v.is_empty() {
            return Some(v);
        }
    }
    CLIENT_ID
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(str::to_string)
}

/// What the UI needs to guide the user through approval. Serialized camelCase
/// for the frontend; `deviceCode` is handed straight back to `poll`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

/// GitHub's device-code response (snake_case wire form).
#[derive(Debug, Deserialize)]
struct DeviceCodeResp {
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    interval: u64,
}

/// GitHub's token-endpoint response: either an `access_token` or an `error`
/// such as `authorization_pending` / `slow_down` / `expired_token`.
#[derive(Debug, Deserialize)]
struct TokenResp {
    access_token: Option<String>,
    error: Option<String>,
    interval: Option<u64>,
}

fn http() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent("fastdash")
        .build()
        .map_err(|e| e.to_string())
}

/// Begin a device-flow login: fetch a code pair and open the browser to the
/// verification page. Returns the codes the UI shows while polling.
pub async fn start() -> Result<DeviceCode, String> {
    let client_id = client_id().ok_or_else(|| {
        "GitHub OAuth App client id is not configured. Register an OAuth App \
         (enable Device Flow) and set its Client ID in device_flow.rs or the \
         FASTDASH_GITHUB_CLIENT_ID env var."
            .to_string()
    })?;

    let resp = http()?
        .post(DEVICE_CODE_URL)
        .header(reqwest::header::ACCEPT, "application/json")
        .form(&[("client_id", client_id.as_str()), ("scope", SCOPES)])
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        return Err(format!("GitHub returned status {}", resp.status().as_u16()));
    }

    let body: DeviceCodeResp = resp.json().await.map_err(|e| e.to_string())?;

    // Best-effort: open the verification page. If it fails (headless, no
    // browser), the UI still shows the link so the user can open it manually.
    let _ = open::that(&body.verification_uri);

    Ok(DeviceCode {
        device_code: body.device_code,
        user_code: body.user_code,
        verification_uri: body.verification_uri,
        expires_in: body.expires_in,
        interval: body.interval,
    })
}

/// Long-poll the token endpoint until the user approves. Honors GitHub's
/// `interval` and `slow_down` back-off. Returns the access token on success.
pub async fn poll(device_code: &str, interval: u64) -> Result<String, String> {
    let client_id = client_id().ok_or_else(|| "GitHub client id not configured".to_string())?;

    // GitHub's minimum is 5s; never poll faster than the server asks.
    let mut wait = interval.max(5);
    let deadline = Instant::now() + Duration::from_secs(MAX_POLL_SECS);

    loop {
        tokio::time::sleep(Duration::from_secs(wait)).await;
        if Instant::now() >= deadline {
            return Err("The login code expired before it was approved. Please try again.".into());
        }

        let resp = http()?
            .post(TOKEN_URL)
            .header(reqwest::header::ACCEPT, "application/json")
            .form(&[
                ("client_id", client_id.as_str()),
                ("device_code", device_code),
                ("grant_type", GRANT_TYPE),
            ])
            .send()
            .await
            .map_err(|e| e.to_string())?;

        let body: TokenResp = resp.json().await.map_err(|e| e.to_string())?;

        if let Some(token) = body.access_token {
            return Ok(token);
        }

        match body.error.as_deref() {
            // Still waiting on the user - keep polling at the current cadence.
            Some("authorization_pending") => {}
            // GitHub asks us to back off; adopt the new interval it returns.
            Some("slow_down") => wait = body.interval.unwrap_or(wait + 5).max(wait + 5),
            Some("expired_token") => {
                return Err("The login code expired before it was approved. Please try again.".into())
            }
            Some("access_denied") => return Err("Access was denied on GitHub.".into()),
            Some(other) => return Err(format!("GitHub device-flow error: {other}")),
            None => return Err("GitHub returned an unexpected empty response.".into()),
        }
    }
}

/// The authenticated user's login, used to confirm the connection in the UI.
#[derive(Debug, Deserialize)]
struct GhUser {
    login: String,
}

/// Verify a freshly minted token and return the account's `login`.
pub async fn fetch_login(token: &str) -> Result<String, String> {
    let resp = http()?
        .get(USER_URL)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        return Err(format!("GitHub returned status {}", resp.status().as_u16()));
    }
    let user: GhUser = resp.json().await.map_err(|e| e.to_string())?;
    Ok(user.login)
}
