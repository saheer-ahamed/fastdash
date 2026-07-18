//! The plug-in contract every connector implements.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::engine::panel::Panel;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorMeta {
    pub id: String,
    pub name: String,
    /// Frontend icon key (not a path).
    pub icon: String,
    pub default_refresh_secs: u64,
}

/// Connector health, surfaced as a status badge in the UI.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum Health {
    Ok,
    NeedsAuth { message: String },
    RateLimited { retry_after_secs: Option<u64> },
    Error { message: String },
}

/// One fetch result: what to render plus status and refresh hints.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub status: Health,
    pub panels: Vec<Panel>,
    pub fetched_at: DateTime<Utc>,
    pub next_refresh_secs: Option<u64>,
}

impl Snapshot {
    pub fn ok(panels: Vec<Panel>, next_refresh_secs: Option<u64>) -> Self {
        Snapshot {
            status: Health::Ok,
            panels,
            fetched_at: Utc::now(),
            next_refresh_secs,
        }
    }

    pub fn needs_auth(message: impl Into<String>) -> Self {
        Snapshot {
            status: Health::NeedsAuth {
                message: message.into(),
            },
            panels: vec![],
            fetched_at: Utc::now(),
            next_refresh_secs: None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConnectorError {
    #[error("authentication required: {0}")]
    Auth(String),
    #[error("rate limited")]
    RateLimited,
    #[error("{0}")]
    Other(String),
}

/// Per-fetch context handed to every connector (timezone for "today", etc.).
#[derive(Debug, Clone)]
pub struct FetchCtx {
    pub timezone: String,
}

#[async_trait]
pub trait Connector: Send + Sync {
    fn meta(&self) -> ConnectorMeta;

    /// Fetch the latest snapshot. Called by the scheduler on the connector's
    /// own cadence and by the UI on manual refresh.
    async fn fetch(&self, ctx: &FetchCtx) -> Result<Snapshot, ConnectorError>;
}
