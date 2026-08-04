//! GitHub connector configuration.
//!
//! Resolution: the first GitHub account configured in the shared `engine::config`
//! (its label + selected orgs), with the token read from the OS keychain via
//! `engine::secrets` (key `github/<label>`, written by the Connectors UI). The env
//! vars `GITHUB_TOKEN` / `FASTDASH_GITHUB_ORGS` / `FASTDASH_GITHUB_LABEL` act as
//! fallbacks for local/dev use.
//!
//! What the token has to carry (the README's "Connecting GitHub" section says the
//! same thing to users, and the Connectors UI repeats it at the token field):
//!
//! * **Classic PAT / Device Flow**: `repo` for private org PRs, `read:org` to
//!   resolve the account's orgs, and `read:user` for private activity in the
//!   contribution heatmap. Missing `read:user` is silent - GitHub reports a total
//!   of 0 rather than an error - so `incomplete_scopes` in `mod.rs` warns instead.
//! * **Fine-grained PAT**: resource owner set to the org, plus read-only
//!   Metadata, Pull requests, Issues, and Contents. There is no fine-grained
//!   equivalent of `read:user`, so the heatmap stays public-only, and one token
//!   covers exactly one resource owner - several orgs need several accounts.
//!
//! TODO: support multiple accounts at once (work `saheer-zro` + personal
//! `saheer-ahamed`) and populate the org checklist from `/user/orgs`.

/// Default account label when none is configured.
const DEFAULT_LABEL: &str = "default";

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
    /// Resolve the configuration for the connector's default view: the first
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

        // Full org set for the account, else the env-var default. Normalized on
        // the way in so every entry downstream holds the no-whitespace
        // invariant, whatever wrote the config.
        let all_orgs: Vec<String> = account
            .map(|a| a.orgs)
            .filter(|o| !o.is_empty())
            .unwrap_or_else(orgs_from_env)
            .iter()
            .map(normalize_scope)
            .filter(|o| !o.is_empty())
            .collect();

        // A specific org narrows the fetch to just that org; otherwise use all.
        let orgs = match org.map(normalize_scope) {
            Some(o) if !o.is_empty() => vec![o],
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

/// Parse `FASTDASH_GITHUB_ORGS` (comma-separated).
///
/// An empty result stays empty on purpose. This used to fall back to a
/// hardcoded org, which meant anyone who cleared the field silently queried
/// somebody else's org and got an opaque 422; the caller now reports "nothing
/// configured" instead.
fn orgs_from_env() -> Vec<String> {
    let raw = std::env::var("FASTDASH_GITHUB_ORGS").unwrap_or_default();
    raw.split(',')
        .map(normalize_scope)
        .filter(|s| !s.is_empty())
        .collect()
}

/// Canonical form of a configured scope entry: no whitespace anywhere.
///
/// Neither a GitHub login nor a qualifier can contain whitespace, so stripping
/// it is lossless - and it is what lets the stored entry be used verbatim
/// everywhere downstream. `scope_qualifier` trims at query-build time, but the
/// raw string is also the chip label and part of the frontend's `viewKey`,
/// whose cache keys are space-separated; a stored `user: octocat` would make
/// that separator ambiguous. Normalizing here holds the invariant for every
/// writer - the Connectors UI, `FASTDASH_GITHUB_ORGS`, a hand-edited config.
fn normalize_scope(entry: impl AsRef<str>) -> String {
    entry
        .as_ref()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The invariant the chip label and the frontend's space-separated
    /// `viewKey` both rely on: a stored entry never contains whitespace.
    #[test]
    fn scopes_are_stored_without_whitespace() {
        assert_eq!(normalize_scope("  acme  "), "acme");
        assert_eq!(normalize_scope("user: octocat"), "user:octocat");
        assert_eq!(normalize_scope(" author : octocat "), "author:octocat");
        assert_eq!(normalize_scope("   "), "");
    }
}
