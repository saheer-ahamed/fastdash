//! Claude usage connector.
//!
//! Reads local `~/.claude/projects/**/*.jsonl` transcripts (tokens, model,
//! effort, timestamps) and overlays the official `/usage` numbers. Fully
//! offline for token/effort/cost; the official limit + reset is best-effort.
//!
//! Owned modules (fleshed out in the `feat/claude` worktree):
//!   - `parse`     JSONL reader (cold full-scan; incremental watcher is a TODO)
//!   - `aggregate` rollups by model / effort / day / week / 5h block
//!   - `usage_api` official /usage pull with offline-estimate fallback
//!   - `pricing`   token -> notional cost

mod aggregate;
mod parse;
mod pricing;
mod usage_api;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::engine::connector::{Connector, ConnectorError, ConnectorMeta, FetchCtx, Snapshot};
use crate::engine::panel::{Bar, Cell, Column, Panel, Stat, TableSpec};

use aggregate::Aggregate;
use usage_api::{OfficialUsage, UsageWindow};

const REFRESH_SECS: u64 = 5;

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
            default_refresh_secs: REFRESH_SECS,
        }
    }

    async fn fetch(&self, _ctx: &FetchCtx) -> Result<Snapshot, ConnectorError> {
        // 1. Official usage (best-effort). Any failure -> local estimate later.
        let official = match usage_api::read_oauth_token() {
            Ok(token) => usage_api::fetch_official_usage(&token).await.ok(),
            Err(_) => None,
        };

        // 2. Local transcripts (blocking fs) off the async runtime.
        let turns = tokio::task::spawn_blocking(parse::scan_transcripts)
            .await
            .map_err(|e| ConnectorError::Other(format!("scan task failed: {e}")))?
            .map_err(|e| ConnectorError::Other(e.to_string()))?;

        let now = Utc::now();
        let agg = aggregate::build(&turns, now);

        let panels = build_panels(&agg, official.as_ref(), now);
        Ok(Snapshot::ok(panels, Some(REFRESH_SECS)))
    }
}

// --- panel assembly ---

fn build_panels(agg: &Aggregate, official: Option<&OfficialUsage>, now: DateTime<Utc>) -> Vec<Panel> {
    let mut panels = Vec::new();

    // 5-hour + weekly meters (official percent, or a local token-based fallback).
    panels.push(five_hour_meter(agg, official, now));
    panels.push(weekly_meter(agg, official, now));

    // KPI tiles.
    let cost: f64 = agg
        .per_model
        .iter()
        .map(|m| pricing::cost_for(&m.model, m.input, m.output, m.cache_read, m.cache_write))
        .sum();

    panels.push(Panel::StatCards {
        title: Some("Claude usage".into()),
        stats: vec![
            Stat {
                label: "Total tokens".into(),
                value: fmt_tokens(agg.total_tokens()),
                sub: Some(format!(
                    "{} in \u{00b7} {} out",
                    fmt_tokens(agg.total_input),
                    fmt_tokens(agg.total_output)
                )),
            },
            Stat {
                label: "Equivalent cost".into(),
                value: fmt_usd(cost),
                sub: Some("notional API rate".into()),
            },
            Stat {
                label: "Sessions".into(),
                value: fmt_count(agg.sessions as u64),
                sub: None,
            },
            Stat {
                label: "Messages".into(),
                value: fmt_count(agg.messages as u64),
                sub: None,
            },
        ],
    });

    // Per-model token table.
    panels.push(tokens_by_model_table(agg));

    // Effort split as fractional bars.
    panels.push(effort_bars(agg));

    // Per-model weekly scoped limits, if the official API returned any.
    if let Some(o) = official {
        if !o.scoped.is_empty() {
            panels.push(scoped_limits_table(o, now));
        }
    }

    panels
}

fn five_hour_meter(agg: &Aggregate, official: Option<&OfficialUsage>, now: DateTime<Utc>) -> Panel {
    match official.and_then(|o| o.five_hour.as_ref()) {
        Some(w) => percent_meter("5-hour window", w, now),
        None => Panel::Meter {
            label: "5-hour window".into(),
            used: agg.five_hour_tokens as f64,
            limit: None,
            unit: "tokens".into(),
            caption: Some(format!(
                "{} in the last 5h \u{00b7} official usage unavailable",
                fmt_tokens(agg.five_hour_tokens)
            )),
        },
    }
}

fn weekly_meter(agg: &Aggregate, official: Option<&OfficialUsage>, now: DateTime<Utc>) -> Panel {
    match official.and_then(|o| o.weekly.as_ref()) {
        Some(w) => percent_meter("Weekly", w, now),
        None => Panel::Meter {
            label: "Weekly".into(),
            used: agg.current_week_tokens as f64,
            limit: None,
            unit: "tokens".into(),
            caption: Some(format!(
                "{} this week \u{00b7} official usage unavailable",
                fmt_tokens(agg.current_week_tokens)
            )),
        },
    }
}

/// A 0..100 percent meter with a humanized reset caption.
fn percent_meter(label: &str, w: &UsageWindow, now: DateTime<Utc>) -> Panel {
    let caption = match w.resets_at {
        Some(reset) => format!("{:.0}% - resets in {}", w.percent, humanize_until(reset, now)),
        None => format!("{:.0}%", w.percent),
    };
    Panel::Meter {
        label: label.into(),
        used: w.percent,
        limit: Some(100.0),
        unit: "%".into(),
        caption: Some(caption),
    }
}

fn tokens_by_model_table(agg: &Aggregate) -> Panel {
    let columns = vec![
        Column { key: "model".into(), label: "Model".into(), numeric: false },
        Column { key: "input".into(), label: "Input".into(), numeric: true },
        Column { key: "output".into(), label: "Output".into(), numeric: true },
        Column { key: "cache_read".into(), label: "Cache read".into(), numeric: true },
        Column { key: "total".into(), label: "Total".into(), numeric: true },
        Column { key: "cost".into(), label: "Cost".into(), numeric: true },
    ];

    let rows = agg
        .per_model
        .iter()
        .map(|m| {
            let cost = pricing::cost_for(&m.model, m.input, m.output, m.cache_read, m.cache_write);
            vec![
                cell(short_model(&m.model)),
                cell(fmt_tokens(m.input)),
                cell(fmt_tokens(m.output)),
                cell(fmt_tokens(m.cache_read)),
                cell(fmt_tokens(m.total())),
                cell(fmt_usd(cost)),
            ]
        })
        .collect();

    Panel::Table(TableSpec {
        title: Some("Tokens by model".into()),
        columns,
        rows,
    })
}

fn effort_bars(agg: &Aggregate) -> Panel {
    let denom = agg.total_effort_output().max(1) as f64;
    let bars = agg
        .effort
        .iter()
        .map(|e| {
            let frac = e.output_tokens as f64 / denom;
            Bar {
                label: e.effort.clone(),
                value: frac,
                display: Some(format!("{:.0}% \u{00b7} {} msgs", frac * 100.0, fmt_count(e.turns))),
            }
        })
        .collect();

    Panel::BarList {
        title: Some("Effort (share of output tokens)".into()),
        bars,
    }
}

fn scoped_limits_table(o: &OfficialUsage, now: DateTime<Utc>) -> Panel {
    let columns = vec![
        Column { key: "model".into(), label: "Model".into(), numeric: false },
        Column { key: "weekly".into(), label: "Weekly".into(), numeric: true },
        Column { key: "resets".into(), label: "Resets in".into(), numeric: false },
    ];
    let rows = o
        .scoped
        .iter()
        .map(|s| {
            let resets = s
                .resets_at
                .map(|r| humanize_until(r, now))
                .unwrap_or_else(|| "-".into());
            vec![
                cell(s.label.clone()),
                cell(format!("{:.0}%", s.percent)),
                cell(resets),
            ]
        })
        .collect();

    Panel::Table(TableSpec {
        title: Some("Weekly limits by model".into()),
        columns,
        rows,
    })
}

// --- formatting helpers ---

fn cell(text: String) -> Cell {
    Cell { text, href: None }
}

/// Compact token count: 12.4M, 3.1K, 1.2B.
fn fmt_tokens(n: u64) -> String {
    let f = n as f64;
    if f >= 1e9 {
        format!("{:.1}B", f / 1e9)
    } else if f >= 1e6 {
        format!("{:.1}M", f / 1e6)
    } else if f >= 1e3 {
        format!("{:.1}K", f / 1e3)
    } else {
        n.to_string()
    }
}

/// Plain count with a K/M suffix only when large (sessions, messages).
fn fmt_count(n: u64) -> String {
    let f = n as f64;
    if f >= 1e6 {
        format!("{:.1}M", f / 1e6)
    } else if f >= 10_000.0 {
        format!("{:.1}K", f / 1e3)
    } else {
        n.to_string()
    }
}

/// Compact USD: $12.3K above a thousand, else two decimals.
fn fmt_usd(cost: f64) -> String {
    if cost >= 1000.0 {
        format!("${:.1}K", cost / 1000.0)
    } else {
        format!("${:.2}", cost)
    }
}

/// Short label for a model id, e.g. "claude-opus-4-8" -> "opus-4-8".
fn short_model(model: &str) -> String {
    model.strip_prefix("claude-").unwrap_or(model).to_string()
}

/// Humanized "time until": "3d 4h", "2h14m", "12m", or "now".
fn humanize_until(target: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let secs = (target - now).num_seconds();
    if secs <= 0 {
        return "now".into();
    }
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3_600;
    let mins = (secs % 3_600) / 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h{mins}m")
    } else {
        format!("{mins}m")
    }
}
