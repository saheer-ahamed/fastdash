//! GitHub connector configuration.
//!
//! TEMPORARY source until the core config/keychain (`feat/core`) lands. For now
//! the token comes from the OS keychain entry `fastdash/github/<label>` and, if
//! that is missing, the `GITHUB_TOKEN` env var. Organizations come from the
//! `FASTDASH_GITHUB_ORGS` env var (comma-separated), defaulting to `z-roworld`.
//!
//! TODO(feat/core): replace this with the shared config loader + keychain. The
//! real design supports multiple accounts (work `saheer-zro`, personal
//! `saheer-ahamed`), each with its own PAT in the keychain, and org selection
//! persisted in `%APPDATA%/fastdash/config.toml` (populated from `/user/orgs`).

/// Keychain service under which per-account tokens live (`<service>/<label>`).
const KEYCHAIN_SERVICE: &str = "fastdash/github";
/// Default account label until multi-account config lands.
const DEFAULT_LABEL: &str = "default";
/// Default org when `FASTDASH_GITHUB_ORGS` is unset.
const DEFAULT_ORG: &str = "z-roworld";

/// Resolved, ready-to-use GitHub configuration.
#[derive(Debug, Clone)]
pub struct GithubConfig {
    pub token: String,
    pub orgs: Vec<String>,
    /// Account label the token was resolved for (diagnostics only).
    #[allow(dead_code)]
    pub label: String,
}

impl GithubConfig {
    /// Resolve the configuration, or `None` if no token is available (the
    /// connector then reports `NeedsAuth`).
    pub fn resolve() -> Option<Self> {
        let label =
            std::env::var("FASTDASH_GITHUB_LABEL").unwrap_or_else(|_| DEFAULT_LABEL.to_string());

        // Prefer the keychain; fall back to the env var for local/dev use.
        let token = token_from_keychain(&label).or_else(token_from_env)?;

        Some(GithubConfig {
            token,
            orgs: orgs_from_env(),
            label,
        })
    }
}

/// Read `fastdash/github/<label>` from the OS keychain. Any error (including a
/// missing entry) yields `None` so the caller can fall back to the env var.
fn token_from_keychain(label: &str) -> Option<String> {
    let entry = keyring::Entry::new(KEYCHAIN_SERVICE, label).ok()?;
    let token = entry.get_password().ok()?;
    let token = token.trim().to_string();
    if token.is_empty() {
        None
    } else {
        Some(token)
    }
}

fn token_from_env() -> Option<String> {
    let token = std::env::var("GITHUB_TOKEN").ok()?;
    let token = token.trim().to_string();
    if token.is_empty() {
        None
    } else {
        Some(token)
    }
}

/// Parse `FASTDASH_GITHUB_ORGS` (comma-separated), defaulting to `z-roworld`.
fn orgs_from_env() -> Vec<String> {
    let raw = std::env::var("FASTDASH_GITHUB_ORGS").unwrap_or_default();
    let orgs: Vec<String> = raw
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();
    if orgs.is_empty() {
        vec![DEFAULT_ORG.to_string()]
    } else {
        orgs
    }
}
