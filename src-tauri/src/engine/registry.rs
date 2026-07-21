//! Holds the set of connectors and looks them up by id.

use std::sync::Arc;

use crate::connectors;
use crate::engine::connector::Connector;

pub struct Registry {
    connectors: Vec<Arc<dyn Connector>>,
}

impl Registry {
    pub fn with_default_connectors() -> Self {
        Registry {
            connectors: vec![
                Arc::new(connectors::claude::ClaudeConnector::new()),
                Arc::new(connectors::github::GithubConnector::new()),
            ],
        }
    }

    pub fn all(&self) -> &[Arc<dyn Connector>] {
        &self.connectors
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn Connector>> {
        self.connectors.iter().find(|c| c.meta().id == id).cloned()
    }
}
