//! App-wide, non-secret configuration.
//!
//! Secrets (GitHub PAT, Slack user token, Claude OAuth token) never live here -
//! they go in the OS keychain via `engine::secrets`. This struct is persisted as
//! TOML at `ProjectDirs::from("co","zro","fastdash").config_dir()/config.toml`
//! (on Windows, under `%APPDATA%`). Loading always succeeds by falling back to
//! defaults, and saving is atomic (write-temp-then-rename).

use std::path::PathBuf;

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

/// Root, non-secret configuration. Every connector reads only the slice it needs
/// (GitHub reads `github`, Slack reads `slack`); all of them share `timezone`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// IANA timezone used to define "today" across connectors.
    pub timezone: String,
    /// GitHub accounts and their selected orgs. The PAT for an account lives in
    /// the keychain under `github/{label}`.
    pub github: GithubConfig,
    /// Slack workspaces. The user token for a workspace lives in the keychain
    /// under `slack/{label}`.
    pub slack: SlackConfig,
    /// When true, connectors that surface authors filter out bots
    /// (dependabot and similar).
    pub filter_bots: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            timezone: "Asia/Kolkata".to_string(),
            github: GithubConfig::default(),
            slack: SlackConfig::default(),
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SlackConfig {
    pub workspaces: Vec<SlackWorkspace>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SlackWorkspace {
    /// Human label and keychain key suffix (secret at `slack/{label}`).
    pub label: String,
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
