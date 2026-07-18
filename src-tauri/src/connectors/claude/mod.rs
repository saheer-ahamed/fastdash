//! Claude usage connector.
//!
//! Reads local `~/.claude/projects/**/*.jsonl` transcripts (tokens, model,
//! effort, timestamps) and overlays the official `/usage` numbers. Fully
//! offline for token/effort/cost; the official limit + reset is best-effort.
//!
//! Owned modules (fleshed out in the `feat/claude` worktree):
//!   - `parse`     incremental JSONL reader + file watcher
//!   - `aggregate` rollups by model / effort / day / week / 5h block
//!   - `usage_api` official /usage pull with offline-estimate fallback
//!   - `pricing`   token -> cost

mod aggregate;
mod parse;
mod pricing;
mod usage_api;

use async_trait::async_trait;

use crate::engine::connector::{Connector, ConnectorError, ConnectorMeta, FetchCtx, Snapshot};
use crate::engine::panel::{Panel, Stat};

pub struct ClaudeConnector;

impl ClaudeConnector {
    pub fn new() -> Self {
        ClaudeConnector
    }
}

#[async_trait]
impl Connector for ClaudeConnector {
    fn meta(&self) -> ConnectorMeta {
        ConnectorMeta {
            id: "claude".into(),
            name: "Claude".into(),
            icon: "claude".into(),
            default_refresh_secs: 5,
        }
    }

    async fn fetch(&self, _ctx: &FetchCtx) -> Result<Snapshot, ConnectorError> {
        // TODO(feat/claude): parse transcripts, aggregate, pull /usage.
        Ok(Snapshot::ok(
            vec![Panel::StatCards {
                title: Some("Claude".into()),
                stats: vec![Stat {
                    label: "Status".into(),
                    value: "connector wired".into(),
                    sub: Some("usage aggregation coming next".into()),
                }],
            }],
            Some(5),
        ))
    }
}
