//! GitHub connector.
//!
//! Per selected org, uses the Search API for the date-filtered PR sets
//! (opened / merged / closed-without-merge / still-open for the IST day), then
//! a batched GraphQL enrichment for additions/deletions/state. Emits three
//! tables: PR counts per contributor, line contributions per contributor
//! (based on PRs MERGED today), and the PR list with repos.
//!
//! Supports multiple accounts (work `saheer-zro`, personal `saheer-ahamed`),
//! each with its own PAT in the OS keychain.

use async_trait::async_trait;

use crate::engine::connector::{Connector, ConnectorError, ConnectorMeta, FetchCtx, Snapshot};

pub struct GithubConnector;

impl GithubConnector {
    pub fn new() -> Self {
        GithubConnector
    }
}

#[async_trait]
impl Connector for GithubConnector {
    fn meta(&self) -> ConnectorMeta {
        ConnectorMeta {
            id: "github".into(),
            name: "GitHub".into(),
            icon: "github".into(),
            default_refresh_secs: 60,
        }
    }

    async fn fetch(&self, _ctx: &FetchCtx) -> Result<Snapshot, ConnectorError> {
        // TODO(feat/github): search + GraphQL enrichment, per-contributor rollups.
        Ok(Snapshot::needs_auth(
            "Add a GitHub token and pick organizations in Settings",
        ))
    }
}
