//! Slack user-token resolution.
//!
//! Token resolution order:
//!   1. OS keychain via `engine::secrets` (key `slack/<label>`, written by Settings)
//!   2. env var `SLACK_USER_TOKEN` (handy for local dev / testing)
//!
//! `search.messages` requires a **user token** (`xoxp`), so a resolved token
//! that is not a user token is reported so the connector can guide the user
//! instead of failing opaquely later.
//!
//! TODO(feat/core + feat/slack): replace the fixed `DEFAULT_LABEL` with the
//! workspace label(s) chosen in Settings, and support multiple workspaces
//! (one token per label) rather than a single default.

const ENV_TOKEN: &str = "SLACK_USER_TOKEN";

/// The workspace label used until config wiring exists.
pub const DEFAULT_LABEL: &str = "default";

/// A resolved Slack token plus where it came from (for diagnostics).
pub struct ResolvedToken {
    pub token: String,
}

impl ResolvedToken {
    /// Slack user tokens start with `xoxp-`. Bot (`xoxb-`), app (`xapp-`), and
    /// legacy tokens cannot call `search.messages`.
    pub fn is_user_token(&self) -> bool {
        self.token.starts_with("xoxp-")
    }
}

/// Resolve the Slack token for `label`, or `None` if none is configured.
///
/// Keychain wins over the env var; either being present is enough.
pub fn resolve(label: &str) -> Option<ResolvedToken> {
    if let Some(token) = from_keyring(label) {
        return Some(ResolvedToken { token });
    }
    from_env().map(|token| ResolvedToken { token })
}

fn from_keyring(label: &str) -> Option<String> {
    // Read through the shared keychain wrapper so this matches exactly what the
    // Settings UI writes via `set_secret` (service `fastdash`, key `slack/<label>`).
    let token = crate::engine::secrets::get("slack", label).ok()??;
    let trimmed = token.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn from_env() -> Option<String> {
    match std::env::var(ENV_TOKEN) {
        Ok(v) if !v.trim().is_empty() => Some(v.trim().to_string()),
        _ => None,
    }
}
