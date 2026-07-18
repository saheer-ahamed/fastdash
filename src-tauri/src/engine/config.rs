//! App-wide, non-secret configuration.
//!
//! Secrets (GitHub PAT, Slack user token, Claude OAuth token) never live here -
//! they go in the OS keychain via the `keyring` crate. This struct will be
//! loaded from `%APPDATA%/fastdash/config.toml`; for now it carries defaults.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// IANA timezone used to define "today" across connectors.
    pub timezone: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            timezone: "Asia/Kolkata".to_string(),
        }
    }
}
