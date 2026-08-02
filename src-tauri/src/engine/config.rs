//! App-wide, non-secret configuration.
//!
//! Secrets (GitHub PAT, Claude OAuth token) never live here -
//! they go in the OS keychain via `engine::secrets`. This struct is persisted as
//! TOML at `ProjectDirs::from("co","zro","fastdash").config_dir()/config.toml`
//! (`%APPDATA%` on Windows, `~/Library/Application Support` on macOS). Loading
//! always succeeds by falling back to
//! defaults, and saving is atomic (write-temp-then-rename).

use std::path::PathBuf;

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

/// Root, non-secret configuration. Every connector reads only the slice it needs
/// (GitHub reads `github`, Claude reads `claude`, Sentry reads `sentry`); all of
/// them share `timezone`.
///
/// Serialized camelCase because this struct crosses to the frontend verbatim
/// through `get_config`/`save_config`, and the TypeScript mirror in `types.ts`
/// is camelCase. Multi-word fields silently failed to round-trip before that
/// rename - the UI wrote `filterBots`, serde looked for `filter_bots`, found
/// nothing, and fell back to the default - so `filter_bots` keeps a snake_case
/// alias to load configs written by older builds.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AppConfig {
    /// IANA timezone used to define "today" across connectors.
    pub timezone: String,
    /// UI language (BCP-47-ish code, e.g. "en"). Only English is shipped today.
    pub locale: String,
    /// GitHub accounts and their selected orgs. The PAT for an account lives in
    /// the keychain under `github/{label}`.
    pub github: GithubConfig,
    /// Claude Console connection. The Admin API key itself lives in the
    /// keychain under `claude/console`.
    pub claude: ClaudeConfig,
    /// Sentry connections and their organizations. The auth token for a
    /// connection lives in the keychain under `sentry/{label}`.
    pub sentry: SentryConfig,
    /// When true, connectors that surface authors filter out bots
    /// (dependabot and similar).
    #[serde(alias = "filter_bots")]
    pub filter_bots: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            timezone: "Asia/Kolkata".to_string(),
            locale: "en".to_string(),
            github: GithubConfig::default(),
            claude: ClaudeConfig::default(),
            sentry: SentryConfig::default(),
            filter_bots: true,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GithubConfig {
    pub accounts: Vec<GithubAccount>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GithubAccount {
    /// Human label and keychain key suffix (secret at `github/{label}`).
    pub label: String,
    /// Orgs selected for this account (e.g. `["z-roworld"]`).
    pub orgs: Vec<String>,
}

/// The Claude connector's non-secret state.
///
/// Only the Console side is configurable: the plan meters read Claude Code's
/// own credentials on this machine and need nothing stored here.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ClaudeConfig {
    /// Console organization the stored Admin key belongs to, captured once at
    /// connect time so a fetch never spends a request re-asking who we are.
    /// Empty means no key is connected.
    pub console_org: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SentryConfig {
    pub accounts: Vec<SentryAccount>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SentryAccount {
    /// Human label and keychain key suffix (secret at `sentry/{label}`).
    pub label: String,
    /// Sentry origin: `https://sentry.io`, a region host like
    /// `https://de.sentry.io`, or a self-hosted install. Empty means SaaS.
    pub base_url: String,
    /// Organization slugs to report on. Empty means "every organization this
    /// token can see", discovered at fetch time.
    pub organizations: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("could not determine a config directory for this platform")]
    NoConfigDir,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Serialize(#[from] toml::ser::Error),
}

/// Absolute path to `config.toml`, or `None` if the platform has no config dir.
pub fn config_path() -> Option<PathBuf> {
    ProjectDirs::from("co", "zro", "fastdash").map(|dirs| dirs.config_dir().join("config.toml"))
}

/// Load the config, falling back to defaults on any error (missing file, bad
/// TOML, unreadable path). The UI must never block on config, so this never
/// returns an error - a broken file simply yields defaults.
pub fn load() -> AppConfig {
    let Some(path) = config_path() else {
        return AppConfig::default();
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text).unwrap_or_else(|err| {
            eprintln!("fastdash: ignoring malformed config at {path:?}: {err}");
            AppConfig::default()
        }),
        Err(_) => AppConfig::default(),
    }
}

/// Persist the config atomically: write a sibling temp file then rename over the
/// target so a crash mid-write can never leave a half-written config.
pub fn save(config: &AppConfig) -> Result<(), ConfigError> {
    let path = config_path().ok_or(ConfigError::NoConfigDir)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = toml::to_string_pretty(config)?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A config written before the camelCase rename must still load - and in
    /// particular must not silently drop the GitHub accounts, which would leave
    /// a returning user staring at an empty dashboard and no obvious cause.
    #[test]
    fn snake_case_configs_still_load() {
        let old = r#"
            timezone = "Asia/Kolkata"
            locale = "en"
            filter_bots = false

            [[github.accounts]]
            label = "work"
            orgs = ["z-roworld"]
        "#;

        let cfg: AppConfig = toml::from_str(old).expect("old config must parse");
        assert_eq!(cfg.timezone, "Asia/Kolkata");
        assert!(!cfg.filter_bots, "filter_bots alias did not apply");
        assert_eq!(cfg.github.accounts.len(), 1);
        assert_eq!(cfg.github.accounts[0].label, "work");
        // Absent slice falls back to "no Console key connected".
        assert!(cfg.claude.console_org.is_empty());
    }

    /// The round trip the UI actually performs: `get_config` hands camelCase to
    /// the frontend, `save_config` hands it back. Before the rename this leg
    /// dropped `filterBots` on the floor and the toggle never persisted.
    #[test]
    fn camel_case_round_trips_through_json() {
        let cfg = AppConfig {
            filter_bots: false,
            claude: ClaudeConfig {
                console_org: "Acme Inc".to_string(),
            },
            ..AppConfig::default()
        };

        let json = serde_json::to_string(&cfg).unwrap();
        assert!(json.contains("\"filterBots\""), "{json}");
        assert!(json.contains("\"consoleOrg\""), "{json}");

        let back: AppConfig = serde_json::from_str(&json).unwrap();
        assert!(!back.filter_bots);
        assert_eq!(back.claude.console_org, "Acme Inc");

        // Same leg for a Sentry connection: `baseUrl` is what `types.ts`
        // declares, and a snake_case slip here is invisible at compile time.
        let account = serde_json::to_value(SentryAccount::default()).unwrap();
        for key in ["label", "baseUrl", "organizations"] {
            assert!(account.get(key).is_some(), "missing `{key}` in {account}");
        }
    }

    /// A config from before the Sentry connector existed must still load - the
    /// whole file falls back to defaults on a parse error, so a missing section
    /// would blank every other setting too.
    #[test]
    fn a_config_without_the_sentry_section_still_loads() {
        let cfg: AppConfig = toml::from_str(
            r#"
            timezone = "Asia/Kolkata"
            locale = "en"

            [[github.accounts]]
            label = "work"
            orgs = ["acme"]
            "#,
        )
        .unwrap();
        assert_eq!(cfg.github.accounts.len(), 1);
        assert!(cfg.sentry.accounts.is_empty());
    }

    /// Round-tripping through TOML must not lose a connection's origin - a
    /// self-hosted install is unreachable without it.
    #[test]
    fn sentry_accounts_round_trip_through_toml() {
        let mut cfg = AppConfig::default();
        cfg.sentry.accounts.push(SentryAccount {
            label: "work".into(),
            base_url: "https://sentry.example.com".into(),
            organizations: vec!["acme".into()],
        });

        let text = toml::to_string_pretty(&cfg).unwrap();
        let back: AppConfig = toml::from_str(&text).unwrap();
        assert_eq!(
            back.sentry.accounts[0].base_url,
            "https://sentry.example.com"
        );
        assert_eq!(back.sentry.accounts[0].organizations, vec!["acme"]);
    }
}
