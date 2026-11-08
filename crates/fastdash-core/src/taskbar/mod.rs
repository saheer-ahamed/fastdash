// The taskbar readout: a few live numbers drawn inside the Windows taskbar,
// just left of the notification area, laid out like the clock - so Claude usage
// and today's PRs stay in view while the dashboard is closed or minimized.
//
// This half is platform-neutral: which numbers, and how they read. The drawing
// lives in `win.rs`; on other platforms the readout is simply absent.
//
// Nothing here runs on a timer. The numbers are fetched when the app starts or
// the readout's settings change (the frontend calls `taskbar_refresh`), and
// after that only when the user clicks the readout's refresh icon.

#[cfg(windows)]
mod win;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::connectors::{claude, github};
use crate::engine::i18n;
use crate::engine::range::{self, DateRange};

/// One number the readout can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Metric {
    /// Claude's rolling 5-hour session window, as a percentage used.
    ClaudeSession,
    /// Claude's weekly all-models window, as a percentage used.
    ClaudeWeekly,
    /// PRs the user opened today, across every GitHub account.
    GithubOpened,
    /// PRs the user merged today, across every GitHub account.
    GithubMerged,
}

impl Metric {
    fn is_github(self) -> bool {
        matches!(self, Metric::GithubOpened | Metric::GithubMerged)
    }
}

/// The readout's settings, saved in the app config and edited under Settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TaskbarConfig {
    /// Whether the readout shows at all.
    pub enabled: bool,
    /// What each line shows, top to bottom.
    #[serde(deserialize_with = "known_metrics")]
    pub lines: Vec<Vec<Metric>>,
}

impl Default for TaskbarConfig {
    fn default() -> Self {
        TaskbarConfig {
            enabled: true,
            lines: default_lines(),
        }
    }
}

/// Read `lines`, skipping any metric this build does not know. A config written
/// by a newer build, or one naming a metric since retired, must lose that one
/// metric - not fail to parse, which would reset every other setting with it.
fn known_metrics<'de, D>(de: D) -> Result<Vec<Vec<Metric>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: Vec<Vec<serde_json::Value>> = Deserialize::deserialize(de)?;
    Ok(raw
        .into_iter()
        .map(|line| {
            line.into_iter()
                .filter_map(|m| serde_json::from_value(m).ok())
                .collect()
        })
        .collect())
}

/// The readout's layout: each inner list is one line of text, top to bottom.
/// Two lines, like the clock beside it: Claude above, GitHub below.
pub fn default_lines() -> Vec<Vec<Metric>> {
    vec![
        vec![Metric::ClaudeSession, Metric::ClaudeWeekly],
        vec![Metric::GithubOpened, Metric::GithubMerged],
    ]
}

/// What the connectors answered, as far as the readout cares.
#[derive(Debug, Clone, Copy, Default)]
struct Readings {
    plan: Option<claude::PlanPercents>,
    prs: Option<github::MyPrCounts>,
}

/// The last good GitHub counts and the day they are about. A failed poll keeps
/// showing them rather than blanking the line, but only on the same day:
/// yesterday's counts under today's clock would be a wrong answer, not a stale
/// one.
static LAST_PRS: Mutex<Option<(NaiveDate, github::MyPrCounts)>> = Mutex::new(None);

/// Start drawing the readout. It stays empty, and so invisible, until the first
/// `refresh`. `on_click` runs when the user clicks it.
pub fn start(
    on_open: impl Fn() + Send + Sync + 'static,
    on_refresh: impl Fn() + Send + Sync + 'static,
) {
    #[cfg(windows)]
    win::start(Box::new(on_open), Box::new(on_refresh));
    #[cfg(not(windows))]
    let _ = (on_open, on_refresh);
}

/// Bring the readout in line with `settings`: fetch and redraw it, or hide it
/// when it is turned off.
pub async fn apply(settings: &TaskbarConfig) {
    if settings.enabled {
        refresh(&settings.lines).await;
    } else {
        clear();
    }
}

/// Whether a refresh is under way. A second one asked for meanwhile (a click
/// racing the startup fetch) is dropped: it would spend the same queries to
/// paint the same numbers.
static REFRESHING: AtomicBool = AtomicBool::new(false);

/// Fetch what `lines` asks for and redraw. Only the connectors a line names
/// are asked, so a readout without GitHub metrics spends no Search budget.
async fn refresh(lines: &[Vec<Metric>]) {
    if REFRESHING.swap(true, Ordering::AcqRel) {
        return;
    }
    set_busy(true);
    let wants = |pick: fn(Metric) -> bool| lines.iter().flatten().any(|m| pick(*m));
    let wants_github = wants(Metric::is_github);
    let wants_claude = wants(|m| !m.is_github());

    let today = range::today_ist();
    let (plan, prs) = tokio::join!(
        async {
            if wants_claude {
                claude::plan_percents().await
            } else {
                None
            }
        },
        async {
            if !wants_github {
                return None;
            }
            let last_prs = || LAST_PRS.lock().unwrap_or_else(|e| e.into_inner());
            // Read before the await and never held across it.
            let kept = last_prs().filter(|(day, _)| *day == today).map(|(_, c)| c);
            match github::my_pr_counts(DateRange::today()).await {
                Ok(Some(counts)) => {
                    *last_prs() = Some((today, counts));
                    Some(counts)
                }
                Ok(None) => None,
                Err(e) => {
                    eprintln!("[taskbar] GitHub counts failed: {e}");
                    kept
                }
            }
        },
    );

    show(compose(lines, &Readings { plan, prs }));
    set_busy(false);
    REFRESHING.store(false, Ordering::Release);
}

fn set_busy(busy: bool) {
    #[cfg(windows)]
    win::set_busy(busy);
    #[cfg(not(windows))]
    let _ = busy;
}

/// Hide the readout, for when the user turns it off.
pub fn clear() {
    show(Vec::new());
}

fn show(lines: Vec<String>) {
    #[cfg(windows)]
    win::set_lines(lines);
    #[cfg(not(windows))]
    let _ = lines;
}

/// The text of each line. A metric with no reading is left out, and a line
/// left with nothing is dropped, so an unconnected connector takes no room.
fn compose(lines: &[Vec<Metric>], r: &Readings) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            let mut parts: Vec<String> = Vec::new();
            let mut prefixed = false;
            for &metric in line {
                let Some(mut text) = format_metric(metric, r) else {
                    continue;
                };
                // "PRs" once per line, before the first GitHub number, so a
                // pair reads "PRs 3 opened · 1 merged" rather than repeating it.
                if metric.is_github() && !prefixed {
                    text = format!("{} {text}", i18n::t("taskbar.prs"));
                    prefixed = true;
                }
                parts.push(text);
            }
            parts.join(" \u{b7} ")
        })
        .filter(|l| !l.is_empty())
        .collect()
}

fn format_metric(metric: Metric, r: &Readings) -> Option<String> {
    let pct = |p: Option<f64>| p.map(|p| format!("{}", p.round().clamp(0.0, 100.0) as u32));
    match metric {
        Metric::ClaudeSession => {
            let p = pct(r.plan?.session)?;
            Some(i18n::tf("taskbar.session", &[("pct", &p)]))
        }
        Metric::ClaudeWeekly => {
            let p = pct(r.plan?.weekly)?;
            Some(i18n::tf("taskbar.weekly", &[("pct", &p)]))
        }
        Metric::GithubOpened => {
            let n = r.prs?.opened.to_string();
            Some(i18n::tf("taskbar.opened", &[("n", &n)]))
        }
        Metric::GithubMerged => {
            let n = r.prs?.merged.to_string();
            Some(i18n::tf("taskbar.merged", &[("n", &n)]))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full() -> Readings {
        Readings {
            plan: Some(claude::PlanPercents {
                session: Some(3.4),
                weekly: Some(14.0),
            }),
            prs: Some(github::MyPrCounts {
                opened: 4,
                merged: 2,
            }),
        }
    }

    #[test]
    fn the_default_layout_reads_like_the_clock() {
        assert_eq!(
            compose(&default_lines(), &full()),
            ["Session 3% \u{b7} Week 14%", "PRs 4 opened \u{b7} 2 merged"]
        );
    }

    #[test]
    fn a_missing_connector_takes_no_room() {
        let r = Readings {
            plan: None,
            ..full()
        };
        assert_eq!(
            compose(&default_lines(), &r),
            ["PRs 4 opened \u{b7} 2 merged"]
        );
    }

    #[test]
    fn prs_is_said_once_per_line_before_the_first_github_number() {
        let lines = vec![vec![
            Metric::ClaudeSession,
            Metric::GithubMerged,
            Metric::GithubOpened,
        ]];
        assert_eq!(
            compose(&lines, &full()),
            ["Session 3% \u{b7} PRs 2 merged \u{b7} 4 opened"]
        );
    }

    #[test]
    fn an_unknown_metric_is_dropped_not_fatal() {
        let cfg: TaskbarConfig = toml::from_str(
            r#"
            enabled = false
            lines = [["claudeWeekly", "somethingNew"], ["githubMerged"]]
            "#,
        )
        .unwrap();
        assert!(!cfg.enabled);
        assert_eq!(
            cfg.lines,
            [vec![Metric::ClaudeWeekly], vec![Metric::GithubMerged]]
        );
    }

    #[test]
    fn a_config_without_the_section_gets_the_default() {
        let cfg: crate::engine::config::AppConfig = toml::from_str("locale = \"en\"").unwrap();
        assert_eq!(cfg.taskbar, TaskbarConfig::default());
    }

    #[test]
    fn metrics_round_trip_as_camel_case() {
        let json = serde_json::to_string(&default_lines()).unwrap();
        assert_eq!(
            json,
            r#"[["claudeSession","claudeWeekly"],["githubOpened","githubMerged"]]"#
        );
    }
}
