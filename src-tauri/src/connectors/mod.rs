//! Self-contained connectors. Each is isolated behind the `Connector` trait so
//! it can be built in its own worktree without touching the engine or the UI.

pub mod claude;
pub mod github;

// Temporary diagnostic harness: dumps the exact Snapshot each connector returns
// using the real keychain token, config, transcripts, and live APIs. Run with:
//   cargo test --lib connectors::diag -- --ignored --nocapture
#[cfg(test)]
mod diag {
    use crate::connectors::{claude::ClaudeConnector, github::GithubConnector};
    use crate::engine::connector::{Connector, FetchCtx};

    fn ctx() -> FetchCtx {
        FetchCtx {
            timezone: "Asia/Kolkata".into(),
        }
    }

    #[tokio::test]
    #[ignore]
    async fn dump_claude() {
        match ClaudeConnector::new().fetch(&ctx()).await {
            Ok(s) => println!(
                "CLAUDE_SNAPSHOT:\n{}",
                serde_json::to_string_pretty(&s).unwrap()
            ),
            Err(e) => println!("CLAUDE_ERROR: {e}"),
        }
    }

    #[tokio::test]
    #[ignore]
    async fn dump_github() {
        match GithubConnector::new().fetch(&ctx()).await {
            Ok(s) => println!(
                "GITHUB_SNAPSHOT:\n{}",
                serde_json::to_string_pretty(&s).unwrap()
            ),
            Err(e) => println!("GITHUB_ERROR: {e}"),
        }
    }
}
