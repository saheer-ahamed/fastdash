//! GitHub connector configuration.
//!
//! Resolution: the first GitHub account configured in the shared `engine::config`
//! (its label + selected orgs), with the token read from the OS keychain via
//! `engine::secrets` (key `github/<label>`, written by the Connectors UI). The env
//! vars `GITHUB_TOKEN` / `FASTDASH_GITHUB_ORGS` / `FASTDASH_GITHUB_LABEL` act as
//! fallbacks for local/dev use.
//!
//! TODO: support multiple accounts at once (work `saheer-zro` + personal
//! `saheer-ahamed`) and populate the org checklist from `/user/orgs`.

/// Default account label when none is configured.
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
    /// Resolve the configuration for the scheduler's default view: the first
    /// account configured under Connectors (all its orgs), or `None` if no token is
    /// available (the connector then reports `NeedsAuth`).
    pub fn resolve() -> Option<Self> {
        // Prefer the first account configured under Connectors; fall back to env vars.
        let label = crate::engine::config::load()
            .github
            .accounts
            .into_iter()
            .next()
            .map(|a| a.label)
            .or_else(|| std::env::var("FASTDASH_GITHUB_LABEL").ok())
            .unwrap_or_else(|| DEFAULT_LABEL.to_string());
        Self::for_account(&label, None)
    }

    /// Resolve the configuration for a specific account label, optionally scoped
    /// to a single org (an org-filter sub-tab). Returns `None` when no token is
    /// available for the label. When `org` is `None` the account's full org list
    /// is used (the "All" sub-tab).
    pub fn for_account(label: &str, org: Option<&str>) -> Option<Self> {
        let account = crate::engine::config::load()
            .github
            .accounts
            .into_iter()
            .find(|a| a.label == label);

        // Prefer the keychain; fall back to the env var for local/dev use.
        let token = token_from_keychain(label).or_else(token_from_env)?;

        // Full org set for the account, else the env-var default.
        let all_orgs = account
            .map(|a| a.orgs)
            .filter(|o| !o.is_empty())
            .unwrap_or_else(orgs_from_env);

        // A specific org narrows the fetch to just that org; otherwise use all.
        let orgs = match org {
            Some(o) if !o.trim().is_empty() => vec![o.trim().to_string()],
            _ => all_orgs,
        };

        Some(GithubConfig {
            token,
            orgs,
            label: label.to_string(),
        })
    }
}

/// Read `fastdash/github/<label>` from the OS keychain. Any error (including a
/// missing entry) yields `None` so the caller can fall back to the env var.
fn token_from_keychain(label: &str) -> Option<String> {
    // Read through the shared keychain wrapper so this matches exactly what the
    // Connectors UI writes via `set_secret` (service `fastdash`, key `github/<label>`).
    let token = crate::engine::secrets::get("github", label).ok()??;
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
