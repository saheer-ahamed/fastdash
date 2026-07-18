//! Cold full-scan JSONL parser for `~/.claude/projects` session transcripts.
//!
//! Each line under `~/.claude/projects/**/*.jsonl` is a JSON record. We keep
//! only lines where `type == "assistant"` and `message.usage` exists, and pull
//! out one [`Turn`] per assistant response (timestamp, effort, session, model,
//! and the full token breakdown). Everything is tolerant of missing / null
//! fields so a shape change on Anthropic's side degrades gracefully instead of
//! dropping the whole file.
//!
//! TODO(feat/claude): add a `notify`-based incremental watcher that tails only
//! the bytes appended to the active session file and feeds a running aggregate,
//! keeping this cold full-scan for startup only. A ~850-file cold scan is fine
//! for v1 and is what `fetch()` uses today.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("home directory not found")]
    NoHome,
}

/// One assistant turn distilled from a transcript line.
#[derive(Debug, Clone)]
pub struct Turn {
    pub timestamp: DateTime<Utc>,
    /// Reasoning effort, e.g. "medium" / "xhigh". "unknown" when absent.
    pub effort: String,
    pub session_id: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    /// Total cache-creation (1h + 5m ephemeral) tokens.
    pub cache_write_tokens: u64,
    pub cache_write_1h_tokens: u64,
    pub cache_write_5m_tokens: u64,
    pub service_tier: Option<String>,
}

/// Resolve `~/.claude/projects`.
pub fn projects_dir() -> Result<PathBuf, ParseError> {
    let base = directories::BaseDirs::new().ok_or(ParseError::NoHome)?;
    Ok(base.home_dir().join(".claude").join("projects"))
}

/// Cold full-scan of every `*.jsonl` transcript, returning all assistant turns.
pub fn scan_transcripts() -> Result<Vec<Turn>, ParseError> {
    let root = projects_dir()?;
    let mut files = Vec::new();
    collect_jsonl(&root, &mut files);

    let mut turns = Vec::new();
    for path in &files {
        // A single unreadable file must not abort the scan.
        if let Ok(contents) = std::fs::read_to_string(path) {
            parse_lines(&contents, &mut turns);
        }
    }
    Ok(turns)
}

/// Parse every line of one transcript file into `out`.
pub fn parse_lines(contents: &str, out: &mut Vec<Turn>) {
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Malformed / partially-written lines are skipped, not fatal.
        let raw: RawLine = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if let Some(turn) = raw.into_turn() {
            out.push(turn);
        }
    }
}

fn collect_jsonl(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl(&path, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
}

// --- wire types (tolerant: every field optional / defaulted) ---

#[derive(Debug, Deserialize)]
struct RawLine {
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<String>,
    effort: Option<String>,
    // The authoritative Claude Code field is camelCase `sessionId`. Some lines
    // *also* carry a distinct snake_case `session_id` (a different value), so
    // these must be separate fields - a serde `alias` would raise a
    // "duplicate field" error and drop the whole line. Prefer `sessionId`.
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    #[serde(rename = "session_id")]
    session_id_snake: Option<String>,
    message: Option<RawMessage>,
}

#[derive(Debug, Deserialize)]
struct RawMessage {
    role: Option<String>,
    model: Option<String>,
    usage: Option<RawUsage>,
}

#[derive(Debug, Deserialize)]
struct RawUsage {
    // Option<u64> tolerates both missing and explicit-null values.
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_creation: Option<RawCacheCreation>,
    service_tier: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawCacheCreation {
    ephemeral_1h_input_tokens: Option<u64>,
    ephemeral_5m_input_tokens: Option<u64>,
}

impl RawLine {
    fn into_turn(self) -> Option<Turn> {
        // Only assistant lines that actually carry usage.
        if self.kind.as_deref() != Some("assistant") {
            return None;
        }
        let message = self.message?;
        // Guard on role too, when present, to skip odd shapes.
        if let Some(role) = &message.role {
            if role != "assistant" {
                return None;
            }
        }
        let usage = message.usage?;

        // A timestamp is required to place the turn on the day/week/5h axes.
        let timestamp = DateTime::parse_from_rfc3339(self.timestamp.as_deref()?)
            .ok()?
            .with_timezone(&Utc);

        let cache = usage.cache_creation.unwrap_or(RawCacheCreation {
            ephemeral_1h_input_tokens: None,
            ephemeral_5m_input_tokens: None,
        });
        let cw_1h = cache.ephemeral_1h_input_tokens.unwrap_or(0);
        let cw_5m = cache.ephemeral_5m_input_tokens.unwrap_or(0);
        // Prefer the flat total when present; else fall back to the split sum.
        let cache_write = usage
            .cache_creation_input_tokens
            .unwrap_or_else(|| cw_1h + cw_5m);

        Some(Turn {
            timestamp,
            effort: self.effort.unwrap_or_else(|| "unknown".to_string()),
            session_id: self
                .session_id
                .or(self.session_id_snake)
                .unwrap_or_default(),
            model: message.model.unwrap_or_else(|| "unknown".to_string()),
            input_tokens: usage.input_tokens.unwrap_or(0),
            output_tokens: usage.output_tokens.unwrap_or(0),
            cache_read_tokens: usage.cache_read_input_tokens.unwrap_or(0),
            cache_write_tokens: cache_write,
            cache_write_1h_tokens: cw_1h,
            cache_write_5m_tokens: cw_5m,
            service_tier: usage.service_tier,
        })
    }
}
