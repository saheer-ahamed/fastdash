//! Per-contributor rollups and `Panel` construction from the fetched PR sets.

use std::collections::HashMap;

use chrono::{DateTime, FixedOffset, NaiveDate, Utc};

use crate::engine::i18n;
use crate::engine::panel::{Cell, Column, HeatDay, HeatmapYear, Panel, Stat, TableSpec};
use crate::engine::range::DateRange;

use super::client::ContributionCalendar;

/// Outcome of a PR within the selected window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrState {
    Merged,
    Closed,
    Open,
}

impl PrState {
    fn label(self) -> String {
        match self {
            PrState::Merged => i18n::t("github.state.merged"),
            PrState::Closed => i18n::t("github.state.closed"),
            PrState::Open => i18n::t("github.state.open"),
        }
    }
}

/// One row of the PR list (union of all four search sets).
#[derive(Debug, Clone)]
pub struct PrEntry {
    pub name_with_owner: String,
    pub title: String,
    pub url: String,
    pub author: Option<String>,
    pub state: PrState,
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
    /// The event time driving the "Time" column (merged/closed/created).
    pub at: Option<DateTime<Utc>>,
}

/// Everything the panels are built from. Counts are per-contributor and each
/// bucket is counted independently (a PR may appear in several).
#[derive(Debug, Default)]
pub struct Rollup {
    pub opened: HashMap<String, u64>,
    pub merged: HashMap<String, u64>,
    pub closed: HashMap<String, u64>,
    pub open: HashMap<String, u64>,
    /// PRs merged in the range, per author, with line counts (from GraphQL).
    pub line_contribs: Vec<LineContrib>,
    /// Union of PRs seen in the range, for the PR list table.
    pub pr_list: Vec<PrEntry>,
}

/// The line contribution of a PR merged in the range, attributed to its author.
#[derive(Debug, Clone)]
pub struct LineContrib {
    pub author: String,
    pub additions: u64,
    pub deletions: u64,
}

/// Build the connector's panels: a `StatCards` header, the contribution
/// heatmap (when available), plus the three tables. Every range-scoped panel
/// names the range in its title, so a screenshot is never ambiguous about which
/// days it covers.
pub fn build_panels(
    rollup: &Rollup,
    ist: FixedOffset,
    range: &DateRange,
    contributions: Vec<Panel>,
) -> Vec<Panel> {
    let label = range.label();
    let mut panels = vec![stat_cards(rollup, &label)];
    panels.extend(contributions);
    panels.push(pr_activity_table(rollup, &label));
    panels.push(line_contributions_table(rollup, &label));
    panels.push(pr_list_table(rollup, ist, range, &label));
    panels
}

/// The year grid of the viewer's contributions. Counts and shade levels are
/// GitHub's own, so this matches the calendar on the profile page exactly.
/// `current_year` gets the rolling last-12-months window GitHub itself shows.
pub fn contributions_heatmap(
    login: &str,
    calendars: &[(String, ContributionCalendar)],
    current_year: i32,
) -> Option<Panel> {
    let years: Vec<HeatmapYear> = calendars
        .iter()
        .filter(|(_, cal)| !cal.days.is_empty())
        .map(|(label, cal)| {
            let n = fmt_count(cal.total);
            let rolling = label == &current_year.to_string();
            let key = match (rolling, cal.total) {
                (true, 1) => "github.contributions.summaryLastYearOne",
                (true, _) => "github.contributions.summaryLastYear",
                (false, 1) => "github.contributions.summaryYearOne",
                (false, _) => "github.contributions.summaryYear",
            };
            let summary = i18n::tf(key, &[("n", &n), ("year", label)]);
            HeatmapYear {
                label: label.clone(),
                summary,
                days: cal.days.iter().map(heat_day).collect(),
            }
        })
        .collect();

    if years.is_empty() {
        return None;
    }
    Some(Panel::Heatmap {
        title: Some(i18n::tf("github.contributions.title", &[("login", login)])),
        years,
    })
}

/// Shown above the grid when the token lacks `read:user`: GitHub then counts
/// only public activity, so an otherwise busy year can render near-empty.
pub fn contributions_partial_note(login: &str, scopes: &str) -> Panel {
    Panel::Note {
        title: Some(i18n::tf("github.contributions.title", &[("login", login)])),
        message: i18n::tf(
            "github.contributions.publicOnly",
            &[("scopes", scopes.trim())],
        ),
    }
}

fn heat_day(day: &super::client::ContributionDay) -> HeatDay {
    let when = pretty_date(&day.date);
    let tooltip = match day.count {
        0 => i18n::tf("github.contributions.tooltipNone", &[("date", &when)]),
        1 => i18n::tf("github.contributions.tooltipOne", &[("date", &when)]),
        n => i18n::tf(
            "github.contributions.tooltipMany",
            &[("n", &fmt_count(n)), ("date", &when)],
        ),
    };
    HeatDay {
        date: day.date.clone(),
        level: day.level.min(4),
        tooltip,
    }
}

/// `2026-07-24` -> `Jul 24, 2026`; falls back to the raw string if unparseable.
fn pretty_date(iso: &str) -> String {
    NaiveDate::parse_from_str(iso, "%Y-%m-%d")
        .map(|d| d.format("%b %-d, %Y").to_string())
        .unwrap_or_else(|_| iso.to_string())
}

/// Thousands separators, e.g. `1234` -> `1,234`.
fn fmt_count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn stat_cards(rollup: &Rollup, range_label: &str) -> Panel {
    let total_opened: u64 = rollup.opened.values().sum();
    let total_merged: u64 = rollup.merged.values().sum();

    // Anyone who opened, merged, closed, or has an open PR counts as active.
    let mut active: std::collections::HashSet<&String> = std::collections::HashSet::new();
    for map in [&rollup.opened, &rollup.merged, &rollup.closed, &rollup.open] {
        active.extend(map.keys());
    }

    Panel::StatCards {
        title: Some(range_label.to_string()),
        stats: vec![
            Stat {
                label: i18n::t("github.stats.prsOpened"),
                value: total_opened.to_string(),
                sub: None,
            },
            Stat {
                label: i18n::t("github.stats.prsMerged"),
                value: total_merged.to_string(),
                sub: None,
            },
            Stat {
                label: i18n::t("github.stats.contributorsActive"),
                value: active.len().to_string(),
                sub: None,
            },
        ],
    }
}

fn pr_activity_table(rollup: &Rollup, range_label: &str) -> Panel {
    // Union of contributors across every bucket.
    let mut logins: std::collections::HashSet<&String> = std::collections::HashSet::new();
    for map in [&rollup.opened, &rollup.merged, &rollup.closed, &rollup.open] {
        logins.extend(map.keys());
    }

    let mut rows: Vec<(String, u64, u64, u64, u64)> = logins
        .into_iter()
        .map(|login| {
            let opened = *rollup.opened.get(login).unwrap_or(&0);
            let merged = *rollup.merged.get(login).unwrap_or(&0);
            let closed = *rollup.closed.get(login).unwrap_or(&0);
            let open = *rollup.open.get(login).unwrap_or(&0);
            (login.clone(), opened, merged, closed, open)
        })
        .collect();

    // Sort by merged desc, then total activity desc, then login asc.
    rows.sort_by(|a, b| {
        let ta = a.1 + a.2 + a.3 + a.4;
        let tb = b.1 + b.2 + b.3 + b.4;
        b.2.cmp(&a.2)
            .then(tb.cmp(&ta))
            .then(a.0.to_lowercase().cmp(&b.0.to_lowercase()))
    });

    let table_rows = rows
        .into_iter()
        .map(|(login, opened, merged, closed, open)| {
            vec![
                text(login),
                num(merged),
                num(opened),
                num(closed),
                num(open),
            ]
        })
        .collect();

    Panel::Table(TableSpec {
        title: Some(i18n::tf(
            "github.table.prActivity",
            &[("range", range_label)],
        )),
        // "Created" and "Still open" are one letter apart in spirit but answer
        // different questions - an event in the window vs a state right now -
        // so each header carries the sentence that spells the difference out.
        columns: vec![
            col("contributor", i18n::t("github.column.contributor"), false),
            col("merged", i18n::t("github.column.merged"), true)
                .with_hint(i18n::t("github.column.mergedHint")),
            col("opened", i18n::t("github.column.created"), true)
                .with_hint(i18n::t("github.column.createdHint")),
            col("closed", i18n::t("github.column.closedUnmerged"), true)
                .with_hint(i18n::t("github.column.closedUnmergedHint")),
            col("open", i18n::t("github.column.stillOpen"), true)
                .with_hint(i18n::t("github.column.stillOpenHint")),
        ],
        rows: table_rows,
    })
}

fn line_contributions_table(rollup: &Rollup, range_label: &str) -> Panel {
    // Aggregate the PRs merged in the range, per author.
    let mut by_author: HashMap<&str, (u64, u64, u64)> = HashMap::new();
    for lc in &rollup.line_contribs {
        let e = by_author.entry(lc.author.as_str()).or_insert((0, 0, 0));
        e.0 += lc.additions;
        e.1 += lc.deletions;
        e.2 += 1;
    }

    let mut rows: Vec<(&str, u64, u64, u64)> = by_author
        .into_iter()
        .map(|(author, (adds, dels, prs))| (author, adds, dels, prs))
        .collect();

    // Sort by total (additions + deletions) desc, then additions desc, then
    // author asc.
    rows.sort_by(|a, b| {
        let ta = a.1 + a.2;
        let tb = b.1 + b.2;
        tb.cmp(&ta)
            .then(b.1.cmp(&a.1))
            .then(a.0.to_lowercase().cmp(&b.0.to_lowercase()))
    });

    let table_rows = rows
        .into_iter()
        .map(|(author, adds, dels, prs)| {
            let total = adds + dels;
            let net = adds as i64 - dels as i64;
            vec![
                text(author.to_string()),
                num(total),
                num(adds),
                num(dels),
                keyed(format_net(net), net as f64),
                num(prs),
            ]
        })
        .collect();

    Panel::Table(TableSpec {
        title: Some(i18n::tf(
            "github.table.lineContributions",
            &[("range", range_label)],
        )),
        columns: vec![
            col("contributor", i18n::t("github.column.contributor"), false),
            col("total", i18n::t("github.column.total"), true)
                .with_hint(i18n::t("github.column.totalHint")),
            col("additions", i18n::t("github.column.additions"), true),
            col("deletions", i18n::t("github.column.deletions"), true),
            col("net", i18n::t("github.column.net"), true)
                .with_hint(i18n::t("github.column.netHint")),
            col("prs", i18n::t("github.column.prs"), true)
                .with_hint(i18n::t("github.column.prsHint")),
        ],
        rows: table_rows,
    })
}

fn pr_list_table(rollup: &Rollup, ist: FixedOffset, range: &DateRange, range_label: &str) -> Panel {
    // A single day needs only the clock; a wider range would be unreadable
    // without the date, so the column carries it.
    let time_format = if range.is_single_day() {
        "%H:%M"
    } else {
        "%b %-d, %H:%M"
    };
    let mut entries: Vec<&PrEntry> = rollup.pr_list.iter().collect();
    // Grouped by contributor: sort by author (case-insensitive), then most
    // recent event first within each author. Unknown authors sink to the bottom.
    entries.sort_by(|a, b| {
        let aa = a.author.as_deref().unwrap_or("~~~").to_lowercase();
        let ba = b.author.as_deref().unwrap_or("~~~").to_lowercase();
        aa.cmp(&ba).then(b.at.cmp(&a.at))
    });

    let rows = entries
        .into_iter()
        .map(|pr| {
            let author = pr.author.clone().unwrap_or_else(|| "-".into());
            // Sorting "+120 / -4" by its characters is meaningless; churn
            // (additions + deletions) is what the column reads as - PR size.
            let delta = match (pr.additions, pr.deletions) {
                (Some(a), Some(d)) => keyed(format!("+{a} / -{d}"), (a + d) as f64),
                _ => text("-".into()),
            };
            // Likewise "Jul 24, 14:30" only sorts right as an instant.
            let time = match pr.at {
                Some(t) => keyed(
                    t.with_timezone(&ist).format(time_format).to_string(),
                    t.timestamp() as f64,
                ),
                None => text("-".into()),
            };
            vec![
                text(author),
                text(pr.name_with_owner.clone()),
                link(pr.title.clone(), pr.url.clone()),
                text(pr.state.label()),
                delta,
                time,
            ]
        })
        .collect();

    Panel::Table(TableSpec {
        title: Some(i18n::tf("github.table.prList", &[("range", range_label)])),
        columns: vec![
            col("author", i18n::t("github.column.contributor"), false),
            col("repo", i18n::t("github.column.repo"), false),
            col("title", i18n::t("github.column.title"), false),
            col("state", i18n::t("github.column.state"), false)
                .with_hint(i18n::t("github.column.stateHint")),
            col("delta", i18n::t("github.column.delta"), false)
                .with_hint(i18n::t("github.column.deltaHint")),
            col("time", i18n::t("github.column.time"), false)
                .with_hint(i18n::t("github.column.timeHint")),
        ],
        rows,
    })
}

fn format_net(net: i64) -> String {
    if net > 0 {
        format!("+{net}")
    } else {
        net.to_string()
    }
}

fn col(key: &str, label: impl Into<String>, numeric: bool) -> Column {
    Column::new(key, label, numeric)
}

fn text(s: String) -> Cell {
    Cell {
        text: s,
        href: None,
        sort: None,
    }
}

/// Text that sorts by a value of its own rather than by its characters.
fn keyed(s: String, sort: f64) -> Cell {
    Cell {
        text: s,
        href: None,
        sort: Some(sort),
    }
}

fn num(n: u64) -> Cell {
    Cell {
        text: n.to_string(),
        href: None,
        sort: Some(n as f64),
    }
}

fn link(text: String, href: String) -> Cell {
    Cell {
        text,
        href: Some(href),
        sort: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::range::{self, DateRange};

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    /// One merged PR at 2026-07-24 14:30 IST.
    fn rollup() -> Rollup {
        Rollup {
            pr_list: vec![PrEntry {
                name_with_owner: "acme/api".into(),
                title: "fix: thing".into(),
                url: "https://github.com/acme/api/pull/1".into(),
                author: Some("dev".into()),
                state: PrState::Merged,
                additions: Some(10),
                deletions: Some(2),
                at: Some(
                    DateTime::parse_from_rfc3339("2026-07-24T14:30:00+05:30")
                        .unwrap()
                        .with_timezone(&Utc),
                ),
            }],
            ..Default::default()
        }
    }

    fn pr_list_row(range: &DateRange) -> Vec<Cell> {
        let panel = pr_list_table(&rollup(), range::ist(), range, "x");
        let Panel::Table(spec) = panel else {
            panic!("expected a table")
        };
        spec.rows[0].clone()
    }

    fn time_cell(range: &DateRange) -> String {
        pr_list_row(range).last().unwrap().text.clone()
    }

    /// Within one day the clock is enough; across days the row would be
    /// ambiguous without its date.
    #[test]
    fn time_column_carries_the_date_only_for_multi_day_ranges() {
        let one_day = DateRange {
            start: day(2026, 7, 24),
            end: day(2026, 7, 24),
        };
        assert_eq!(time_cell(&one_day), "14:30");

        let week = DateRange {
            start: day(2026, 7, 21),
            end: day(2026, 7, 27),
        };
        assert_eq!(time_cell(&week), "Jul 24, 14:30");
    }

    /// Columns the frontend can't sort from their text - the delta pair and the
    /// formatted timestamp - must carry an explicit key, or clicking the header
    /// would order them alphabetically.
    #[test]
    fn unsortable_text_columns_carry_a_sort_key() {
        let range = DateRange {
            start: day(2026, 7, 24),
            end: day(2026, 7, 24),
        };
        let row = pr_list_row(&range);

        let delta = &row[4];
        assert_eq!(delta.text, "+10 / -2");
        assert_eq!(delta.sort, Some(12.0), "delta sorts by churn");

        let time = row.last().unwrap();
        let expected = DateTime::parse_from_rfc3339("2026-07-24T14:30:00+05:30")
            .unwrap()
            .timestamp() as f64;
        assert_eq!(time.sort, Some(expected), "time sorts as an instant");

        // Plain text keeps no key and falls back to comparing characters.
        assert_eq!(row[0].sort, None);
    }
}
