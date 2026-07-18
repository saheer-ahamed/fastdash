//! Self-contained connectors. Each is isolated behind the `Connector` trait so
//! it can be built in its own worktree without touching the engine or the UI.

pub mod claude;
pub mod github;
pub mod slack;
