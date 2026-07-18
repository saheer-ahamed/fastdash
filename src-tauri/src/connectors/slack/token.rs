//! Slack user-token resolution.
//!
//! Temporary source until `feat/core` lands proper config + keychain wiring:
//!   1. OS keychain entry with service `fastdash/slack/<label>`
//!   2. env var `SLACK_USER_TOKEN` (handy for local dev / testing)
//!
//! `search.messages` requires a **user token** (`xoxp`), so a resolved token
//! that is not a user token is reported so the connector can guide the user
//! instead of failing opaquely later.
//!
//! TODO(feat/core + feat/slack): replace the fixed `DEFAULT_LABEL` with the
//! workspace label(s) chosen in Settings, and support multiple workspaces
//! (one token per label) rather than a single default.

/// Keychain account name paired with the `fastdash/slack/<label>` service so
/// lookups are deterministic across platforms.
const KEYRING_ACCOUNT: &str = "user-token";
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
    let service = format!("fastdash/slack/{label}");
    // Any keyring failure (no entry, locked keychain, platform error) is treated
    // as "not configured here" so we can fall through to the env var.
    let entry = keyring::Entry::new(&service, KEYRING_ACCOUNT).ok()?;
    let token = entry.get_password().ok()?;
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
