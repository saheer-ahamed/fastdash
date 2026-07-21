//! Claude usage connector.
//!
//! Reads local `~/.claude/projects/**/*.jsonl` transcripts (tokens, model,
//! effort, timestamps) and overlays the official `/usage` numbers. Fully
//! offline for token/effort/cost; the official limit + reset is best-effort.
//!
//! The official `/usage` endpoint is rate-limited, so it is pulled at most once
//! per `OFFICIAL_TTL` and the last good value is reused if a later pull fails
//! (e.g. a transient 429). The local transcript scan drives everything else.
//!
//! Owned modules:
//!   - `parse`     JSONL reader (cold full-scan; incremental watcher is a TODO)
//!   - `aggregate` rollups by model / effort / day / week / month / 5h block
//!   - `usage_api` official /usage pull + plan label
//!   - `pricing`   token -> notional cost

mod aggregate;
mod parse;
mod pricing;
mod usage_api;

use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::{DateTime, FixedOffset, Utc};

use crate::engine::connector::{Connector, ConnectorError, ConnectorMeta, FetchCtx, Snapshot};
use crate::engine::i18n;
use crate::engine::panel::{Bar, Cell, Column, Panel, Stat, TableSpec};

use aggregate::Aggregate;
use usage_api::{OfficialUsage, ScopedLimit, UsageWindow};

/// Local transcript re-scan cadence.
const REFRESH_SECS: u64 = 60;
/// Minimum spacing between official `/usage` pulls (the endpoint is rate-limited
/// and returns 429 if polled too often).
const OFFICIAL_TTL: Duration = Duration::from_secs(45);

pub struct ClaudeConnector {
    /// Last good official usage plus when it was fetched, used to throttle the
    /// pull and to survive a transient rate-limit.
    official: Mutex<Option<(OfficialUsage, Instant)>>,
}

impl ClaudeConnector {
    pub fn new() -> Self {
        ClaudeConnector {
            official: Mutex::new(None),
        }
    }

    /// Official usage, pulling fresh only if the cache is older than
    /// `OFFICIAL_TTL`. On a failed pull the last good value is reused so a 429
    /// never blanks the meters. The lock is never held across the await.
    async fn official_usage(&self) -> Option<OfficialUsage> {
        {
            let guard = self.official.lock().unwrap();
            if let Some((usage, at)) = guard.as_ref() {
                if at.elapsed() < OFFICIAL_TTL {
                    return Some(usage.clone());
                }
            }
        }

        let fresh = match usage_api::read_oauth_token() {
            Ok(token) => usage_api::fetch_official_usage(&token).await.ok(),
            Err(_) => None,
        };

        let mut guard = self.official.lock().unwrap();
        match fresh {
            Some(usage) => {
                *guard = Some((usage.clone(), Instant::now()));
                Some(usage)
            }
            // Offline or rate-limited: reuse the last good value if we have one.
            None => guard.as_ref().map(|(u, _)| u.clone()),
        }
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
        let official = self.official_usage().await;
        let plan = usage_api::read_plan();

        // Local transcripts (blocking fs) off the async runtime.
        let turns = tokio::task::spawn_blocking(parse::scan_transcripts)
            .await
            .map_err(|e| ConnectorError::Other(format!("scan task failed: {e}")))?
            .map_err(|e| ConnectorError::Other(e.to_string()))?;

        let now = Utc::now();
        let agg = aggregate::build(&turns, now);

        let panels = build_panels(&agg, official.as_ref(), plan, now);
        Ok(Snapshot::ok(panels, Some(REFRESH_SECS)))
    }
}

// --- panel assembly ---

/// IST offset for humanized reset times.
fn ist() -> FixedOffset {
    FixedOffset::east_opt(5 * 3600 + 30 * 60).expect("IST offset is valid")
}

fn build_panels(
    agg: &Aggregate,
    official: Option<&OfficialUsage>,
    plan: Option<String>,
    now: DateTime<Utc>,
) -> Vec<Panel> {
    let mut panels = Vec::new();

    // --- Plan usage limits (official numbers) ---
    panels.push(Panel::Heading {
        title: i18n::t("claude.planUsageLimits"),
        badge: plan,
    });

    match official {
        Some(o) => {
            if let Some(w) = &o.five_hour {
                panels.push(limit_meter(&i18n::t("claude.currentSession"), w, now));
            }
            panels.push(Panel::Heading {
                title: i18n::t("claude.weeklyLimits"),
                badge: None,
            });
            if let Some(w) = &o.weekly {
                panels.push(limit_meter(&i18n::t("claude.allModels"), w, now));
            }
            for s in &o.scoped {
                panels.push(scoped_meter(s, now));
            }
        }
        None => {
            // Official numbers unavailable (offline / rate-limited): show local
            // estimates so the section is never blank.
            panels.push(local_meter(
                &i18n::t("claude.currentSessionEst"),
                agg.five_hour_tokens,
                &i18n::t("claude.inLast5h"),
            ));
            panels.push(local_meter(
                &i18n::t("claude.thisWeekEst"),
                agg.current_week_tokens,
                &i18n::t("claude.thisWeek"),
            ));
        }
    }

    // --- Token usage (local transcripts) ---
    panels.push(Panel::Heading {
        title: i18n::t("claude.tokenUsage"),
        badge: None,
    });

    let cost: f64 = agg
        .per_model
        .iter()
        .map(|m| pricing::cost_for(&m.model, m.input, m.output, m.cache_read, m.cache_write))
        .sum();

    panels.push(Panel::StatCards {
        title: None,
        stats: vec![
            Stat {
                label: i18n::t("claude.thisMonth"),
                value: fmt_tokens(agg.current_month_tokens),
                sub: Some(i18n::t("claude.tokens")),
            },
            Stat {
                label: i18n::t("claude.today"),
                value: fmt_tokens(agg.today_tokens),
                sub: Some(i18n::t("claude.tokens")),
            },
            Stat {
                label: i18n::t("claude.allTime"),
                value: fmt_tokens(agg.total_tokens()),
                sub: Some(i18n::tf(
                    "claude.sessions",
                    &[("n", &fmt_count(agg.sessions as u64))],
                )),
            },
            Stat {
                label: i18n::t("claude.equivalentCost"),
                value: fmt_usd(cost),
                sub: Some(i18n::t("claude.allTimeNotional")),
            },
        ],
    });

    panels.push(monthly_table(agg));
    panels.push(tokens_by_model_table(agg));
    panels.push(effort_bars(agg));

    panels
}

/// A 0..100 official limit meter: "31% used" on the right, reset under the label.
fn limit_meter(label: &str, w: &UsageWindow, now: DateTime<Utc>) -> Panel {
    let sub = if w.percent <= 0.0 && w.resets_at.is_none() {
        Some(i18n::tf("claude.notUsedYet", &[("label", label)]))
    } else {
        w.resets_at.map(|r| fmt_reset(r, now))
    };
    Panel::Meter {
        label: label.into(),
        used: w.percent,
        limit: Some(100.0),
        unit: "%".into(),
        sub,
        caption: Some(i18n::tf(
            "claude.percentUsed",
            &[("p", &format!("{:.0}", w.percent))],
        )),
    }
}

fn scoped_meter(s: &ScopedLimit, now: DateTime<Utc>) -> Panel {
    let w = UsageWindow {
        percent: s.percent,
        resets_at: s.resets_at,
    };
    limit_meter(&s.label, &w, now)
}

/// Fallback meter shown when the official numbers are unavailable: a local token
/// count with no percentage bar.
fn local_meter(label: &str, tokens: u64, when: &str) -> Panel {
    Panel::Meter {
        label: label.into(),
        used: 0.0,
        limit: None,
        unit: "tokens".into(),
        sub: Some(i18n::t("claude.localEstimate")),
        caption: Some(i18n::tf(
            "claude.localCaption",
            &[("tokens", &fmt_tokens(tokens)), ("when", when)],
        )),
    }
}

fn monthly_table(agg: &Aggregate) -> Panel {
    let columns = vec![
        Column {
            key: "month".into(),
            label: i18n::t("claude.colMonth"),
            numeric: false,
        },
        Column {
            key: "tokens".into(),
            label: i18n::t("claude.colTokens"),
            numeric: true,
        },
    ];
    let rows = agg
        .per_month
        .iter()
        .map(|m| vec![cell(m.label.clone()), cell(fmt_tokens(m.total_tokens))])
        .collect();

    Panel::Table(TableSpec {
        title: Some(i18n::t("claude.tokensByMonth")),
        columns,
        rows,
    })
}

fn tokens_by_model_table(agg: &Aggregate) -> Panel {
    let columns = vec![
        Column {
            key: "model".into(),
            label: i18n::t("claude.colModel"),
            numeric: false,
        },
        Column {
            key: "input".into(),
            label: i18n::t("claude.colInput"),
            numeric: true,
        },
        Column {
            key: "output".into(),
            label: i18n::t("claude.colOutput"),
            numeric: true,
        },
        Column {
            key: "cache_read".into(),
            label: i18n::t("claude.colCacheRead"),
            numeric: true,
        },
        Column {
            key: "total".into(),
            label: i18n::t("claude.colTotal"),
            numeric: true,
        },
        Column {
            key: "cost".into(),
            label: i18n::t("claude.colCost"),
            numeric: true,
        },
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
        title: Some(i18n::t("claude.tokensByModel")),
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
                display: Some(i18n::tf(
                    "claude.effortDisplay",
                    &[
                        ("p", &format!("{:.0}", frac * 100.0)),
                        ("n", &fmt_count(e.turns)),
                    ],
                )),
            }
        })
        .collect();

    Panel::BarList {
        title: Some(i18n::t("claude.effortTitle")),
        bars,
    }
}

// --- formatting helpers ---

fn cell(text: String) -> Cell {
    Cell { text, href: None }
}

/// "Resets in 47 min" / "Resets in 2h 14m" when soon; else the absolute IST
/// weekday + time, e.g. "Resets Sat 10:30 AM".
fn fmt_reset(target: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let secs = (target - now).num_seconds();
    if secs <= 0 {
        return i18n::t("claude.resetsNow");
    }
    if secs < 12 * 3600 {
        let hours = secs / 3600;
        let mins = (secs % 3600) / 60;
        if hours > 0 {
            i18n::tf(
                "claude.resetsInHm",
                &[("h", &hours.to_string()), ("m", &mins.to_string())],
            )
        } else {
            i18n::tf("claude.resetsInMin", &[("m", &mins.to_string())])
        }
    } else {
        let local = target.with_timezone(&ist());
        i18n::tf(
            "claude.resetsAt",
            &[("when", &local.format("%a %-I:%M %p").to_string())],
        )
    }
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
    if cost < 0.005 {
        return "$0.00".into();
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    // Verifies the official-usage panel layout deterministically, without
    // depending on the (rate-limited) live endpoint.
    #[test]
    fn official_panels_render() {
        let now = Utc::now();
        let official = OfficialUsage {
            five_hour: Some(UsageWindow {
                percent: 31.0,
                resets_at: Some(now + chrono::Duration::minutes(47)),
            }),
            weekly: Some(UsageWindow {
                percent: 3.0,
                resets_at: Some(now + chrono::Duration::days(6)),
            }),
            scoped: vec![ScopedLimit {
                label: "Fable".into(),
                percent: 0.0,
                resets_at: None,
            }],
        };
        let panels = build_panels(
            &Aggregate::default(),
            Some(&official),
            Some("Max (5x)".into()),
            now,
        );
        println!(
            "PANELS:\n{}",
            serde_json::to_string_pretty(&panels).unwrap()
        );
        assert!(panels.len() >= 6);
    }
}
