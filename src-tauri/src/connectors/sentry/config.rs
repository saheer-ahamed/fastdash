//! Sentry connector configuration.
//!
//! Resolution: the accounts configured in the shared `engine::config` (label,
//! base URL, organization slugs), each with its token read from the OS keychain
//! via `engine::secrets` (key `sentry/<label>`, written by the Connectors UI).
//! The env vars `SENTRY_AUTH_TOKEN` / `FASTDASH_SENTRY_URL` /
//! `FASTDASH_SENTRY_ORGS` act as fallbacks for local/dev use.
//!
//! What the token has to carry (the README's "Connecting Sentry" section says
//! the same thing to users):
//!
//! * **User auth token** - the one to use. Settings -> Account -> User Auth
//!   Tokens, with `org:read`, `project:read` and `event:read`. It can see every
//!   organization the account belongs to.
//! * **Internal integration token** - equivalent, scoped to one organization,
//!   and the right choice for a shared/team setup.
//! * **Organization auth token** (`sntrys_`) - built for CI (releases and
//!   source maps) and does not carry `event:read`, so the issue stream comes
//!   back 403. `mod.rs` says so in the banner rather than reporting a generic
//!   permissions error.

use crate::engine::config::SentryAccount;

/// Sentry SaaS. A region-pinned org uses `https://<region>.sentry.io`; a
/// self-hosted install uses its own origin.
pub const DEFAULT_BASE_URL: &str = "https://sentry.io";
/// Account label used when only the env vars are set.
const DEFAULT_LABEL: &str = "default";

/// One configured connection, with its token resolved.
#[derive(Debug, Clone)]
pub struct SentryConnection {
    /// Account label and keychain key suffix (secret at `sentry/{label}`).
    pub label: String,
    pub token: String,
    /// Origin only, e.g. `https://sentry.io`; the client appends `/api/0`.
    pub base_url: String,
    /// Organization slugs to report on. Empty means "every organization this
    /// token can see", discovered at fetch time.
    pub organizations: Vec<String>,
}

/// Every connection that has both a label and a token, in configured order.
///
/// An account without a token is skipped rather than reported: it is a row the
/// user is still filling in, and failing the whole connector over it would hide
/// the accounts that do work. When nothing at all is configured the env vars
/// are used, so a dev run needs no UI.
pub fn connections() -> Vec<SentryConnection> {
    let accounts = crate::engine::config::load().sentry.accounts;
    let resolved: Vec<SentryConnection> = accounts.iter().filter_map(resolve).collect();
    if resolved.is_empty() {
        return from_env().into_iter().collect();
    }
    resolved
}

fn resolve(account: &SentryAccount) -> Option<SentryConnection> {
    let label = account.label.trim();
    if label.is_empty() {
        return None;
    }
    let token = token_from_keychain(label).or_else(token_from_env)?;
    Some(SentryConnection {
        label: label.to_string(),
        token,
        base_url: base_url_or_default(&account.base_url),
        organizations: normalize_orgs(&account.organizations),
    })
}

/// The env-var connection, for `cargo test` and `npm run tauri dev` without
/// touching the keychain.
fn from_env() -> Option<SentryConnection> {
    let token = token_from_env()?;
    Some(SentryConnection {
        label: std::env::var("FASTDASH_SENTRY_LABEL")
            .ok()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .unwrap_or_else(|| DEFAULT_LABEL.to_string()),
        token,
        base_url: base_url_or_default(&std::env::var("FASTDASH_SENTRY_URL").unwrap_or_default()),
        organizations: normalize_orgs(
            &std::env::var("FASTDASH_SENTRY_ORGS")
                .unwrap_or_default()
                .split(',')
                .map(str::to_string)
                .collect::<Vec<_>>(),
        ),
    })
}

/// Read `fastdash/sentry/<label>` from the OS keychain. Any error (including a
/// missing entry) yields `None` so the caller can fall back to the env var.
fn token_from_keychain(label: &str) -> Option<String> {
    let token = crate::engine::secrets::get("sentry", label).ok()??;
    non_empty(token)
}

fn token_from_env() -> Option<String> {
    non_empty(std::env::var("SENTRY_AUTH_TOKEN").ok()?)
}

fn non_empty(s: String) -> Option<String> {
    let s = s.trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn base_url_or_default(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        DEFAULT_BASE_URL.to_string()
    } else {
        raw.to_string()
    }
}

/// Canonical form of the org list: lower-cased, whitespace-free slugs, deduped,
/// order preserved.
///
/// A slug is lower-case by construction on Sentry's side, but the field is
/// free text and users paste the display name ("Acme Inc") or a URL fragment.
/// Folding case here means `Acme` and `acme` cannot render the same
/// organization twice, and the org section headings stay stable.
fn normalize_orgs(raw: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    raw.iter()
        .map(|o| {
            o.chars()
                .filter(|c| !c.is_whitespace())
                .flat_map(char::to_lowercase)
                .collect::<String>()
        })
        .filter(|o| !o.is_empty())
        .filter(|o| seen.insert(o.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn org_slugs_are_normalized_and_deduped() {
        let raw = [" Acme ", "acme", "my-team", "", "   ", "My-Team"].map(String::from);
        assert_eq!(normalize_orgs(&raw), vec!["acme", "my-team"]);
    }

    #[test]
    fn a_blank_base_url_falls_back_to_saas() {
        assert_eq!(base_url_or_default("  "), DEFAULT_BASE_URL);
        assert_eq!(
            base_url_or_default(" https://de.sentry.io "),
            "https://de.sentry.io"
        );
    }

    /// An account row with no label is one the user is still typing; it must
    /// not resolve into a fetch against an empty keychain key.
    #[test]
    fn unlabelled_accounts_do_not_resolve() {
        let account = SentryAccount {
            label: "   ".into(),
            base_url: DEFAULT_BASE_URL.into(),
            organizations: vec!["acme".into()],
        };
        assert!(resolve(&account).is_none());
    }
}
