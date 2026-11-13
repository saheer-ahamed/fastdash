//! JSONL parser for `~/.claude/projects` session transcripts.
//!
//! Each line under `~/.claude/projects/**/*.jsonl` is a JSON record. We keep
//! only lines where `type == "assistant"` and `message.usage` exists, and pull
//! out one [`Turn`] per assistant response (timestamp, effort, session, model,
//! and the full token breakdown). Everything is tolerant of missing / null
//! fields so a shape change on Anthropic's side degrades gracefully instead of
//! dropping the whole file.
//!
//! Fetches are on demand, so there is no file watcher. Each scan stats every
//! transcript and reuses the turns already parsed for a file whose length and
//! mtime have not moved. A file that only grew is read from the previous end
//! when its first bytes still match, which is how Claude Code writes a session.
//! The parsed turns are also written under the app cache dir so the next launch
//! does not reread history that is already on disk.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::engine::perf::{self, CacheState};

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("home directory not found")]
    NoHome,
}

/// One assistant turn distilled from a transcript line.
#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// Resolve `~/.claude`, everything Claude Code keeps on this machine.
pub fn claude_dir() -> Result<PathBuf, ParseError> {
    let base = directories::BaseDirs::new().ok_or(ParseError::NoHome)?;
    Ok(base.home_dir().join(".claude"))
}

/// Resolve `~/.claude/projects`.
pub fn projects_dir() -> Result<PathBuf, ParseError> {
    Ok(claude_dir()?.join("projects"))
}

/// Every assistant turn under `~/.claude/projects`, reusing files that have not
/// changed since the last scan.
pub fn scan_transcripts() -> Result<Vec<Turn>, ParseError> {
    let root = projects_dir()?;
    let started = Instant::now();
    let mut guard = global_index()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    load_index(&mut guard);
    let report = scan_index(&root, &mut guard);
    if report.dirty {
        save_index(&guard);
    }
    let cache = if report.full == 0 && report.tailed == 0 {
        CacheState::Hit
    } else if report.reused > 0 || report.tailed > 0 {
        CacheState::Delta
    } else {
        CacheState::Miss
    };
    drop(guard);
    perf::record(
        "claude.transcripts",
        started.elapsed(),
        report.bytes_read,
        cache,
        true,
    );
    Ok(report.turns)
}

struct FileEntry {
    len: u64,
    mtime_ms: u64,
    /// Hash of the first 64 bytes. An append does not change it; a rewrite does,
    /// which is how a grown file that is not a pure append falls back to a full
    /// read instead of parsing a torn tail.
    head: u64,
    turns: Vec<Turn>,
}

struct TranscriptIndex {
    files: HashMap<PathBuf, FileEntry>,
    loaded: bool,
}

impl TranscriptIndex {
    fn new() -> Self {
        Self {
            files: HashMap::new(),
            loaded: false,
        }
    }
}

struct ScanReport {
    turns: Vec<Turn>,
    bytes_read: u64,
    reused: u32,
    tailed: u32,
    full: u32,
    dirty: bool,
}

enum FileRead {
    Reused(Vec<Turn>),
    Parsed {
        entry: FileEntry,
        bytes: u64,
        tailed: bool,
    },
    /// Missing or unreadable. The scan skips it; one bad file is not fatal.
    Skip,
}

fn global_index() -> &'static Mutex<TranscriptIndex> {
    static INDEX: std::sync::OnceLock<Mutex<TranscriptIndex>> = std::sync::OnceLock::new();
    INDEX.get_or_init(|| Mutex::new(TranscriptIndex::new()))
}

fn persist_enabled() -> bool {
    !cfg!(test)
}

fn index_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("co", "zro", "fastdash")
        .map(|dirs| dirs.cache_dir().join("claude-transcripts.json"))
}

fn load_index(index: &mut TranscriptIndex) {
    if index.loaded || !persist_enabled() {
        index.loaded = true;
        return;
    }
    index.loaded = true;
    let Some(path) = index_path() else {
        return;
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let Ok(disk) = serde_json::from_str::<DiskIndex>(&text) else {
        return;
    };
    for file in disk.files {
        index.files.insert(
            PathBuf::from(file.path),
            FileEntry {
                len: file.len,
                mtime_ms: file.mtime_ms,
                head: file.head,
                turns: file.turns,
            },
        );
    }
}

fn save_index(index: &TranscriptIndex) {
    if !persist_enabled() {
        return;
    }
    let Some(path) = index_path() else {
        return;
    };
    let disk = DiskIndex {
        files: index
            .files
            .iter()
            .map(|(path, entry)| DiskFile {
                path: path.to_string_lossy().into_owned(),
                len: entry.len,
                mtime_ms: entry.mtime_ms,
                head: entry.head,
                turns: entry.turns.clone(),
            })
            .collect(),
    };
    if let Err(err) = write_json(&path, &disk) {
        eprintln!("claude: could not persist transcript index: {err}");
    }
}

fn write_json(path: &Path, value: &impl Serialize) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string(value).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct DiskIndex {
    files: Vec<DiskFile>,
}

#[derive(Serialize, Deserialize)]
struct DiskFile {
    path: String,
    len: u64,
    mtime_ms: u64,
    head: u64,
    turns: Vec<Turn>,
}

fn scan_index(root: &Path, index: &mut TranscriptIndex) -> ScanReport {
    let mut files = Vec::new();
    collect_jsonl(root, &mut files);

    let mut turns = Vec::new();
    let mut seen = HashSet::new();
    let mut reused = 0u32;
    let mut tailed = 0u32;
    let mut full = 0u32;
    let mut bytes_read = 0u64;
    let mut dirty = false;

    for path in files {
        seen.insert(path.clone());
        match read_file(&path, index) {
            FileRead::Reused(existing) => {
                reused += 1;
                turns.extend(existing);
            }
            FileRead::Parsed {
                entry,
                bytes,
                tailed: is_tail,
            } => {
                bytes_read += bytes;
                dirty = true;
                if is_tail {
                    tailed += 1;
                } else {
                    full += 1;
                }
                turns.extend(entry.turns.iter().cloned());
                index.files.insert(path, entry);
            }
            FileRead::Skip => {}
        }
    }

    let before = index.files.len();
    index.files.retain(|path, _| seen.contains(path));
    if index.files.len() != before {
        dirty = true;
    }

    ScanReport {
        turns,
        bytes_read,
        reused,
        tailed,
        full,
        dirty,
    }
}

fn read_file(path: &Path, index: &TranscriptIndex) -> FileRead {
    let Ok(meta) = std::fs::metadata(path) else {
        return FileRead::Skip;
    };
    let len = meta.len();
    let mtime_ms = mtime_ms(&meta);
    if let Some(entry) = index.files.get(path) {
        if entry.len == len && entry.mtime_ms == mtime_ms {
            return FileRead::Reused(entry.turns.clone());
        }
        if len > entry.len {
            if let Some(parsed) = read_tail(path, entry, len, mtime_ms) {
                return parsed;
            }
        }
    }
    match std::fs::read(path) {
        Ok(bytes) => FileRead::Parsed {
            entry: FileEntry {
                len,
                mtime_ms,
                head: head_hash(&bytes),
                turns: turns_in(&bytes),
            },
            bytes: bytes.len() as u64,
            tailed: false,
        },
        Err(_) => FileRead::Skip,
    }
}

/// Parse only the bytes appended past `entry.len`, when the head of the file
/// is still the one we parsed and the previous end was a line boundary.
fn read_tail(path: &Path, entry: &FileEntry, len: u64, mtime_ms: u64) -> Option<FileRead> {
    if entry.len == 0 {
        return None;
    }
    let mut file = std::fs::File::open(path).ok()?;
    let mut head = [0u8; 64];
    let n = file.read(&mut head).ok()?;
    let head_len = n.min(64);
    // A file that was shorter than the head window changes its hash as it
    // grows, so the stored head cannot vouch for the tail. Those are small
    // enough to reread. A longer file must still start with the same bytes.
    if entry.len < 64 || head_hash(&head[..head_len]) != entry.head {
        return None;
    }
    file.seek(SeekFrom::Start(entry.len - 1)).ok()?;
    let mut last = [0u8; 1];
    file.read_exact(&mut last).ok()?;
    if last[0] != b'\n' {
        return None;
    }
    file.seek(SeekFrom::Start(entry.len)).ok()?;
    let mut tail = Vec::new();
    file.read_to_end(&mut tail).ok()?;
    let text = std::str::from_utf8(&tail).ok()?;
    let mut extra = Vec::new();
    parse_lines(text, &mut extra);
    let mut turns = entry.turns.clone();
    turns.extend(extra);
    Some(FileRead::Parsed {
        entry: FileEntry {
            len,
            mtime_ms,
            head: entry.head,
            turns,
        },
        bytes: head_len as u64 + 1 + tail.len() as u64,
        tailed: true,
    })
}

fn turns_in(bytes: &[u8]) -> Vec<Turn> {
    let mut turns = Vec::new();
    if let Ok(text) = std::str::from_utf8(bytes) {
        parse_lines(text, &mut turns);
    }
    turns
}

fn head_hash(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let n = bytes.len().min(64);
    bytes[..n].hash(&mut hasher);
    hasher.finish()
}

fn mtime_ms(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
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

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::time::{Duration, Instant};

    use super::*;

    fn line(n: u32) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"2026-08-02T10:00:00Z","sessionId":"s{n}","effort":"medium","message":{{"role":"assistant","model":"claude-sonnet","usage":{{"input_tokens":{n},"output_tokens":3,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}}}}"#
        )
    }

    /// What `fetch` used to do: read and parse every byte of every file.
    fn full_reread(root: &Path) -> (Vec<Turn>, u64, Duration) {
        let started = Instant::now();
        let mut files = Vec::new();
        collect_jsonl(root, &mut files);
        let mut turns = Vec::new();
        let mut bytes = 0u64;
        for path in files {
            let Ok(text) = std::fs::read_to_string(path) else {
                continue;
            };
            bytes += text.len() as u64;
            parse_lines(&text, &mut turns);
        }
        (turns, bytes, started.elapsed())
    }

    #[test]
    fn an_unchanged_transcript_is_not_reread_and_an_append_reads_the_tail() {
        let root =
            std::env::temp_dir().join(format!("fastdash-transcripts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let session = root.join("projects").join("demo");
        std::fs::create_dir_all(&session).unwrap();

        // Enough files, each well past the 64-byte head, that a later append
        // can be a tail read rather than a rewrite of a short file.
        const FILES: u32 = 60;
        const LINES: u32 = 200;
        for file in 0..FILES {
            let mut body = String::new();
            for n in 0..LINES {
                body.push_str(&line(file * LINES + n));
                body.push('\n');
            }
            std::fs::write(session.join(format!("s{file}.jsonl")), body).unwrap();
        }

        let (full_turns, full_bytes, full_time) = full_reread(&root);
        let mut index = TranscriptIndex::new();
        let cold_started = Instant::now();
        let cold = scan_index(&root, &mut index);
        let cold_time = cold_started.elapsed();
        assert_eq!(cold.turns.len(), full_turns.len());
        assert_eq!(cold.turns.len(), (FILES * LINES) as usize);
        assert!(cold.bytes_read > 0);
        assert_eq!(cold.full, FILES);

        let warm_started = Instant::now();
        let warm = scan_index(&root, &mut index);
        let warm_time = warm_started.elapsed();
        assert_eq!(warm.turns.len(), cold.turns.len());
        assert_eq!(warm.bytes_read, 0, "unchanged files must not be reread");
        assert_eq!(warm.reused, FILES);
        assert_eq!(warm.full, 0);
        assert_eq!(warm.tailed, 0);

        let path = session.join("s0.jsonl");
        let mut appended = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(appended, "{}", line(9_000_000)).unwrap();
        drop(appended);

        let tail = scan_index(&root, &mut index);
        assert_eq!(tail.turns.len(), cold.turns.len() + 1);
        assert_eq!(tail.tailed, 1);
        assert_eq!(tail.full, 0);
        assert!(
            tail.bytes_read < full_bytes / 10,
            "tail read {} bytes, full reread was {full_bytes}",
            tail.bytes_read
        );
        let inputs: Vec<u64> = tail.turns.iter().map(|t| t.input_tokens).collect();
        assert!(inputs.contains(&9_000_000));

        // A rewrite (not an append) must not reuse the stale turns.
        std::fs::write(&path, format!("{}\n", line(8_000_000))).unwrap();
        let rewritten = scan_index(&root, &mut index);
        assert_eq!(rewritten.full, 1);
        let inputs: Vec<u64> = rewritten.turns.iter().map(|t| t.input_tokens).collect();
        assert!(inputs.contains(&8_000_000));
        assert!(!inputs.contains(&9_000_000));

        eprintln!(
            "claude transcripts: full reread {full_bytes} bytes in {}ms, cold scan {}ms, warm scan {}ms (0 bytes), tail {} bytes",
            full_time.as_millis(),
            cold_time.as_millis(),
            warm_time.as_millis(),
            tail.bytes_read
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
