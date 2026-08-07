//! Claude usage connector.
//!
//! Two sources, each used for what only it can answer:
//!
//! * **Anthropic Console, via the Admin API** (`admin_api`) - the connected
//!   account's official token counts and real USD spend over the selected
//!   range. This is the authenticated half: the user connects an Admin API key
//!   in Connectors -> Claude and it is the source of truth for the token-usage
//!   panels whenever it returns anything.
//! * **This machine's Claude Code state** - the `~/.claude/projects/**/*.jsonl`
//!   transcripts (effort split, all-time history) and the `/usage` plan meters.
//!   Anthropic exposes no API for a Pro/Max plan's 5h and weekly limits, so the
//!   meters can only come from here.
//!
//! Why not an OAuth "Connect with Claude" button mirroring the GitHub one:
//! Anthropic operates no third-party OAuth client registration, and since
//! February 2026 their policy reserves subscription OAuth tokens for Claude
//! Code and claude.ai. A browser authorize flow would have to present Claude
//! Code's own client id as ours. The Admin API key is the sanctioned path;
//! `admin_api` documents what it does and does not cover.
//!
//! The UI's date filter scopes the token-usage panels (defaulting to today);
//! the plan-limit meters keep showing the live session/weekly windows, since
//! those are current-state readings rather than a report over past days.
//!
//! The `/usage` endpoint is rate-limited, so it is pulled at most once per
//! `OFFICIAL_TTL` and the last good value is reused if a later pull fails
//! (e.g. a transient 429).
//!
//! Owned modules:
//!   - `parse`     JSONL reader (cold full-scan; incremental watcher is a TODO)
//!   - `aggregate` rollups by model / effort / day / week / month / 5h block
//!   - `usage_api` plan-limit meters + plan label, from Claude Code's own login
//!   - `admin_api` official Console usage + cost, from the connected Admin key
//!   - `pricing`   token -> notional cost, the fallback when Console has no data

pub mod admin_api;
mod aggregate;
mod parse;
mod pricing;
mod usage_api;

use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::engine::config::AppConfig;
use crate::engine::connector::{Connector, ConnectorError, ConnectorMeta, FetchCtx, Snapshot};
use crate::engine::i18n;
use crate::engine::panel::{Bar, Cell, Column, Panel, Stat, TableSpec};
use crate::engine::range::{self, DateRange};
use crate::engine::secrets;

use admin_api::ConsoleUsage;
use aggregate::Aggregate;
use usage_api::{OfficialUsage, ScopedLimit, UsageWindow};

/// Local transcript re-scan cadence.
const REFRESH_SECS: u64 = 60;
/// Minimum spacing between official `/usage` pulls (the endpoint is rate-limited
/// and returns 429 if polled too often).
const OFFICIAL_TTL: Duration = Duration::from_secs(45);
/// Keychain label for the Console Admin API key (stored at `claude/console`).
pub const CONSOLE_LABEL: &str = "console";

/// What the Console half of the fetch produced. The three outcomes render
/// differently on purpose: real numbers, an explanation, or a warning - never a
/// row of zeros, which on a usage dashboard reads as "you did nothing".
enum Console {
    /// A key is connected and reported usage.
    Usage(ConsoleUsage),
    /// A key is connected and works, but this range has no Console-billed
    /// usage. Ordinary for a Pro/Max-only account; explained rather than shown.
    Empty,
    /// A key is connected but the call failed. Local numbers stand in.
    Failed(String),
}

/// The Admin API key the user connected, if any.
fn console_key() -> Option<String> {
    secrets::get("claude", CONSOLE_LABEL)
        .ok()
        .flatten()
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
}

/// Whether Claude Code has left anything on this machine to read. Its login and
/// its transcripts both live under `~/.claude`, so the directory existing is
/// what says the plan meters and the local panels have a source.
fn has_local_state() -> bool {
    parse::claude_dir().map(|p| p.exists()).unwrap_or(false)
}

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

    /// Either source alone is enough, because either alone renders a dashboard:
    /// `fetch` never reports `NeedsAuth`, and the Console key is enrichment on
    /// top of what Claude Code already left on this machine.
    ///
    /// Deliberately not `cfg.claude.console_org`, and not the key alone.
    /// Anthropic issues Admin keys only to Console organizations, so an
    /// individual Pro/Max account can never connect one - keying the tab on it
    /// would hide the app's headline dashboard from exactly the people it was
    /// built for, permanently. `console_org` is a display mirror written after
    /// the key is stored, which makes it a second source of truth that a failed
    /// patch or a credential cleared in the OS store desynchronises.
    fn is_configured(&self, _cfg: &AppConfig) -> bool {
        console_key().is_some() || has_local_state()
    }

    async fn fetch(&self, ctx: &FetchCtx) -> Result<Snapshot, ConnectorError> {
        let official = self.official_usage().await;
        let plan = usage_api::read_plan();
        let range = ctx.range.normalized();

        // Official Console numbers, when a key is connected. A failure here is
        // never fatal: the local scan below can still render the dashboard.
        let console = match console_key() {
            None => None,
            Some(key) => Some(match admin_api::fetch_console_usage(&key, &range).await {
                Ok(usage) if usage.is_empty() => Console::Empty,
                Ok(usage) => Console::Usage(usage),
                Err(e) => {
                    eprintln!("claude: Console usage unavailable: {e}");
                    Console::Failed(e.to_string())
                }
            }),
        };

        // Local transcripts (blocking fs) off the async runtime.
        let turns = tokio::task::spawn_blocking(parse::scan_transcripts)
            .await
            .map_err(|e| ConnectorError::Other(format!("scan task failed: {e}")))?
            .map_err(|e| ConnectorError::Other(e.to_string()))?;

        let now = Utc::now();
        let agg = aggregate::build(&turns, now, &range);

        let panels = build_panels(&agg, official.as_ref(), console.as_ref(), plan, now, &range);
        Ok(Snapshot::ok(panels, Some(REFRESH_SECS)))
    }
}

// --- panel assembly ---

fn build_panels(
    agg: &Aggregate,
    official: Option<&OfficialUsage>,
    console: Option<&Console>,
    plan: Option<String>,
    now: DateTime<Utc>,
    range: &DateRange,
) -> Vec<Panel> {
    let mut panels = Vec::new();
    let range_label = range.label();

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

    // --- Token usage ---
    //
    // Console is the source of truth whenever it has data for the range: those
    // are billed numbers rather than a local transcript scan priced against a
    // hand-maintained table. Otherwise the local scan stands in, preceded by a
    // note saying why - a connected key that reports nothing is a fact about
    // the account (Pro/Max usage is not Console-billed), not a fetch failure,
    // and it deserves a sentence rather than a row of zeros.
    match console {
        Some(Console::Usage(usage)) => {
            panels.push(Panel::Heading {
                title: i18n::t("claude.tokenUsage"),
                badge: Some(i18n::t("claude.sourceConsole")),
            });
            panels.push(console_stats(usage, &range_label));
            panels.push(console_model_table(usage, &range_label));
        }
        other => {
            match other {
                Some(Console::Empty) => panels.push(Panel::Note {
                    title: Some(i18n::t("claude.consoleEmptyTitle")),
                    message: i18n::tf("claude.consoleEmpty", &[("range", &range_label)]),
                }),
                Some(Console::Failed(message)) => panels.push(Panel::Note {
                    title: Some(i18n::t("claude.consoleFailedTitle")),
                    message: i18n::tf("claude.consoleFailed", &[("error", message)]),
                }),
                _ => {}
            }
            panels.push(Panel::Heading {
                title: i18n::t("claude.tokenUsage"),
                badge: Some(i18n::t("claude.sourceLocal")),
            });
            panels.push(local_stats(agg, &range_label));
            panels.push(tokens_by_model_table(agg, &range_label));
        }
    }

    // Both of these are local-only by necessity: the Console report caps at 31
    // buckets (so it cannot carry all-time history) and has no effort
    // dimension at all.
    panels.push(monthly_table(agg));
    panels.push(effort_bars(agg, &range_label));

    panels
}

/// The headline tiles when Console has the range covered: billed tokens and
/// real spend, not transcript estimates priced against `pricing.rs`.
fn console_stats(usage: &ConsoleUsage, range_label: &str) -> Panel {
    let output: u64 = usage.per_model.iter().map(|m| m.output).sum();
    let cost = match usage.cost_usd {
        Some(c) => fmt_usd(c),
        None => i18n::t("claude.costUnavailable"),
    };

    Panel::StatCards {
        title: None,
        stats: vec![
            Stat {
                label: range_label.into(),
                value: fmt_tokens(usage.total_tokens()),
                sub: Some(i18n::t("claude.tokens")),
            },
            Stat {
                label: i18n::t("claude.actualCost"),
                value: cost,
                sub: Some(i18n::tf("claude.rangeBilled", &[("range", range_label)])),
            },
            Stat {
                label: i18n::t("claude.colOutput"),
                value: fmt_tokens(output),
                sub: Some(i18n::t("claude.tokens")),
            },
            Stat {
                label: i18n::t("claude.modelsUsed"),
                value: fmt_count(usage.per_model.len() as u64),
                sub: Some(i18n::tf("claude.inRange", &[("range", range_label)])),
            },
        ],
    }
}

/// The local-transcript headline tiles, used when no Console data covers the
/// range. Cost here is notional - `pricing.rs` list prices applied to scanned
/// tokens - which is why the tile is labelled differently to the Console one.
fn local_stats(agg: &Aggregate, range_label: &str) -> Panel {
    let cost: f64 = agg
        .per_model
        .iter()
        .map(|m| pricing::cost_for(&m.model, m.input, m.output, m.cache_read, m.cache_write))
        .sum();

    Panel::StatCards {
        title: None,
        stats: vec![
            Stat {
                label: range_label.into(),
                value: fmt_tokens(agg.range_tokens),
                sub: Some(i18n::tf(
                    "claude.sessions",
                    &[("n", &fmt_count(agg.sessions as u64))],
                )),
            },
            Stat {
                label: i18n::t("claude.equivalentCost"),
                value: fmt_usd(cost),
                sub: Some(i18n::tf("claude.rangeNotional", &[("range", range_label)])),
            },
            Stat {
                label: i18n::t("claude.thisMonth"),
                value: fmt_tokens(agg.current_month_tokens),
                sub: Some(i18n::t("claude.tokens")),
            },
            Stat {
                label: i18n::t("claude.allTime"),
                value: fmt_tokens(agg.all_time_tokens),
                sub: Some(i18n::tf(
                    "claude.sessions",
                    &[("n", &fmt_count(agg.all_time_sessions as u64))],
                )),
            },
        ],
    }
}

/// Per-model billed tokens. No cost column: the cost report groups by
/// description rather than by model, and splitting it per row would mean
/// parsing prose like "Claude Sonnet 4 Usage - Input Tokens". The billed total
/// sits in the stat tile above, where it is exact.
fn console_model_table(usage: &ConsoleUsage, range_label: &str) -> Panel {
    let columns = vec![
        Column::new("model", i18n::t("claude.colModel"), false),
        Column::new("input", i18n::t("claude.colInput"), true)
            .with_hint(i18n::t("claude.hintUncached")),
        Column::new("output", i18n::t("claude.colOutput"), true),
        Column::new("cache_read", i18n::t("claude.colCacheRead"), true),
        Column::new("cache_write", i18n::t("claude.colCacheWrite"), true),
        Column::new("total", i18n::t("claude.colTotal"), true),
    ];

    let rows = usage
        .per_model
        .iter()
        .map(|m| {
            vec![
                cell(short_model(&m.model)),
                keyed(fmt_tokens(m.uncached_input), m.uncached_input as f64),
                keyed(fmt_tokens(m.output), m.output as f64),
                keyed(fmt_tokens(m.cache_read), m.cache_read as f64),
                keyed(fmt_tokens(m.cache_write), m.cache_write as f64),
                keyed(fmt_tokens(m.total()), m.total() as f64),
            ]
        })
        .collect();

    Panel::Table(TableSpec {
        title: Some(i18n::tf("claude.tokensByModel", &[("range", range_label)])),
        columns,
        rows,
    })
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
        Column::new("month", i18n::t("claude.colMonth"), false),
        Column::new("tokens", i18n::t("claude.colTokens"), true),
    ];
    let rows = agg
        .per_month
        .iter()
        .map(|m| {
            vec![
                cell(m.label.clone()),
                keyed(fmt_tokens(m.total_tokens), m.total_tokens as f64),
            ]
        })
        .collect();

    Panel::Table(TableSpec {
        title: Some(i18n::t("claude.tokensByMonth")),
        columns,
        rows,
    })
}

fn tokens_by_model_table(agg: &Aggregate, range_label: &str) -> Panel {
    let columns = vec![
        Column::new("model", i18n::t("claude.colModel"), false),
        Column::new("input", i18n::t("claude.colInput"), true),
        Column::new("output", i18n::t("claude.colOutput"), true),
        Column::new("cache_read", i18n::t("claude.colCacheRead"), true),
        Column::new("total", i18n::t("claude.colTotal"), true),
        Column::new("cost", i18n::t("claude.colCost"), true),
    ];

    let rows = agg
        .per_model
        .iter()
        .map(|m| {
            let cost = pricing::cost_for(&m.model, m.input, m.output, m.cache_read, m.cache_write);
            vec![
                cell(short_model(&m.model)),
                keyed(fmt_tokens(m.input), m.input as f64),
                keyed(fmt_tokens(m.output), m.output as f64),
                keyed(fmt_tokens(m.cache_read), m.cache_read as f64),
                keyed(fmt_tokens(m.total()), m.total() as f64),
                keyed(fmt_usd(cost), cost),
            ]
        })
        .collect();

    Panel::Table(TableSpec {
        title: Some(i18n::tf("claude.tokensByModel", &[("range", range_label)])),
        columns,
        rows,
    })
}

fn effort_bars(agg: &Aggregate, range_label: &str) -> Panel {
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
        title: Some(i18n::tf("claude.effortTitle", &[("range", range_label)])),
        bars,
    }
}

// --- formatting helpers ---

fn cell(text: String) -> Cell {
    Cell {
        text,
        href: None,
        sort: None,
    }
}

/// A cell whose display text ("1.2M", "$3.40") must sort by its real value.
fn keyed(text: String, sort: f64) -> Cell {
    Cell {
        text,
        href: None,
        sort: Some(sort),
    }
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
        let local = target.with_timezone(&range::ist());
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
            None,
            Some("Max (5x)".into()),
            now,
            &DateRange::today(),
        );
        println!(
            "PANELS:\n{}",
            serde_json::to_string_pretty(&panels).unwrap()
        );
        assert!(panels.len() >= 6);
    }

    fn panels_with(console: Option<&Console>) -> String {
        let panels = build_panels(
            &Aggregate::default(),
            None,
            console,
            None,
            Utc::now(),
            &DateRange::today(),
        );
        serde_json::to_string(&panels).unwrap()
    }

    fn sample_usage() -> ConsoleUsage {
        ConsoleUsage {
            per_model: vec![admin_api::ModelUsage {
                model: "claude-opus-5".into(),
                uncached_input: 1_500,
                cache_read: 200,
                cache_write: 1_000,
                output: 500,
            }],
            cost_usd: Some(1.2345),
        }
    }

    /// With Console data the panels must carry the billed figures, and must not
    /// also carry the notional "Equivalent cost" tile - two cost numbers from
    /// two different methods on one screen is the confusing outcome.
    #[test]
    fn console_usage_replaces_the_local_estimate() {
        let json = panels_with(Some(&Console::Usage(sample_usage())));
        assert!(json.contains("$1.23"), "billed cost missing: {json}");
        assert!(
            !json.contains("Equivalent cost"),
            "notional cost tile survived alongside the billed one: {json}"
        );
    }

    /// A connected key with nothing to report must say so in words. Falling
    /// through to zeros would read as "you used nothing today", when the real
    /// meaning is "your Pro/Max usage is not billed through Console".
    #[test]
    fn empty_console_explains_itself_and_falls_back() {
        let json = panels_with(Some(&Console::Empty));
        assert!(json.contains("\"kind\":\"note\""), "{json}");
        assert!(
            json.contains("Equivalent cost"),
            "local panels must still render: {json}"
        );
    }

    /// A failed call surfaces the reason and still renders the dashboard from
    /// local data - a Console outage must not blank the Claude tab.
    #[test]
    fn failed_console_surfaces_the_error_and_falls_back() {
        let json = panels_with(Some(&Console::Failed("boom".into())));
        assert!(json.contains("boom"), "{json}");
        assert!(json.contains("Equivalent cost"), "{json}");
    }

    /// Every string these branches reach for has to resolve; `i18n::t` returns
    /// the key itself when one is missing, so a typo ships as `claude.foo`.
    #[test]
    fn console_copy_resolves() {
        for key in [
            "claude.sourceConsole",
            "claude.sourceLocal",
            "claude.consoleEmptyTitle",
            "claude.consoleFailedTitle",
            "claude.actualCost",
            "claude.costUnavailable",
            "claude.modelsUsed",
            "claude.colCacheWrite",
            "claude.hintUncached",
        ] {
            let value = i18n::t(key);
            assert_ne!(value, key, "untranslated: {key}");
            assert!(!value.is_empty(), "empty: {key}");
        }

        let empty = i18n::tf("claude.consoleEmpty", &[("range", "Today")]);
        assert!(empty.contains("Today"), "{empty}");
        assert!(!empty.contains("{range}"), "{empty}");

        let failed = i18n::tf("claude.consoleFailed", &[("error", "boom")]);
        assert!(failed.contains("boom"), "{failed}");
        assert!(!failed.contains("{error}"), "{failed}");

        let billed = i18n::tf("claude.rangeBilled", &[("range", "Today")]);
        assert!(billed.contains("Today"), "{billed}");
        assert!(!billed.contains("{range}"), "{billed}");

        let in_range = i18n::tf("claude.inRange", &[("range", "Today")]);
        assert!(in_range.contains("Today"), "{in_range}");
        assert!(!in_range.contains("{range}"), "{in_range}");
    }

    /// The tab must not hang off `claude.consoleOrg`. That field only mirrors
    /// the stored key, and an individual account can never obtain one at all -
    /// a config-driven answer would leave those users staring at an empty
    /// sidebar forever.
    #[test]
    fn the_config_does_not_decide_whether_claude_is_configured() {
        let connector = ClaudeConnector::new();
        let mut cfg = AppConfig::default();
        let without_org = connector.is_configured(&cfg);

        cfg.claude.console_org = "Acme Inc".into();
        assert_eq!(
            connector.is_configured(&cfg),
            without_org,
            "the console org in the config moved the answer",
        );
    }

    /// Local state is looked for where Claude Code actually keeps it: one
    /// directory up from the transcripts `scan_transcripts` reads, so a machine
    /// that has run Claude Code can never read as unconnected.
    #[test]
    fn local_state_is_the_directory_the_transcripts_live_under() {
        let claude = parse::claude_dir().unwrap();
        assert!(claude.ends_with(".claude"), "{claude:?}");
        assert_eq!(parse::projects_dir().unwrap(), claude.join("projects"));
    }
}
