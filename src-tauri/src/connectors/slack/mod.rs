//! Slack connector.
//!
//! Per workspace, resolves the current user via `auth.test`, then uses
//! `search.messages` (`<@me> after:<today>`) to find messages that mention me
//! today, grouped by channel. Requires a user token (`xoxp`) with `search:read`
//! - bot tokens cannot search. Token lives in the OS keychain.

use async_trait::async_trait;

use crate::engine::connector::{Connector, ConnectorError, ConnectorMeta, FetchCtx, Snapshot};

pub struct SlackConnector;

impl SlackConnector {
    pub fn new() -> Self {
        SlackConnector
    }
}

#[async_trait]
impl Connector for SlackConnector {
    fn meta(&self) -> ConnectorMeta {
        ConnectorMeta {
            id: "slack".into(),
            name: "Slack".into(),
            icon: "slack".into(),
            default_refresh_secs: 60,
        }
    }

    async fn fetch(&self, _ctx: &FetchCtx) -> Result<Snapshot, ConnectorError> {
        // TODO(feat/slack): auth.test + search.messages, group by channel.
        Ok(Snapshot::needs_auth(
            "Add a Slack user token and pick a workspace in Settings",
        ))
    }
}
