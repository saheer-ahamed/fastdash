//! Per-organization rollups and `Panel` construction from the fetched issues.

use std::collections::HashMap;

use chrono::{DateTime, FixedOffset, Utc};

use crate::engine::i18n;
use crate::engine::panel::{Bar, Cell, Column, Panel, Stat, TableSpec};
use crate::engine::range::DateRange;

use super::client::Issue;

/// Projects shown in the "events by project" breakdown before it stops being a
/// glance and starts being a second table.
const TOP_PROJECTS: usize = 8;

/// One organization's slice of the dashboard.
#[derive(Debug, Clone)]
pub struct OrgReport {
    /// Account label the organization was fetched through.
    pub account: String,
    pub org: String,
    /// Sorted most events first by the API (`sort=freq`).
    pub issues: Vec<Issue>,
    /// Sentry had more pages than the client follows, so the totals are a floor.
    pub truncated: bool,
}

/// How the range is applied to `firstSeen`, and how timestamps are rendered.
#[derive(Debug, Clone, Copy)]
pub struct Window {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub tz: FixedOffset,
}

impl Window {
    fn contains(&self, at: Option<DateTime<Utc>>) -> bool {
        matches!(at, Some(at) if at >= self.start && at <= self.end)
    }
}

/// Build the connector's panels: one section per organization, each with a
/// `StatCards` header, the per-project breakdown, and the issue table. Every
/// panel names the range in its title, so a screenshot is never ambiguous about
/// which days it covers.
///
/// The section heading only appears when there is more than one section - with
/// a single organization it would be a second title above a dashboard that
/// already says which connector it is.
pub fn build_panels(reports: &[OrgReport], range: &DateRange, window: Window) -> Vec<Panel> {
    let label = range.label();
    let sectioned = reports.len() > 1;
    let accounts: std::collections::HashSet<&String> = reports.iter().map(|r| &r.account).collect();
    let multi_account = accounts.len() > 1;

    let mut panels = Vec::new();
    for report in reports {
        if sectioned {
            panels.push(Panel::Heading {
                title: report.org.clone(),
                // Which connection it came through only disambiguates when
                // there is more than one.
                badge: multi_account.then(|| report.account.clone()),
            });
        }
        panels.push(stat_cards(report, &label, window));
        if let Some(bars) = events_by_project(report) {
            panels.push(bars);
        }
        panels.push(match issue_table(report, range, window) {
            Some(table) => table,
            None => Panel::Note {
                title: Some(i18n::tf("sentry.table.issues", &[("range", &label)])),
                message: i18n::t("sentry.empty"),
            },
        });
        if report.truncated {
            panels.push(Panel::Note {
                title: Some(i18n::t("sentry.truncatedTitle")),
                message: i18n::tf(
                    "sentry.truncated",
                    &[("n", &report.issues.len().to_string())],
                ),
            });
        }
    }
    panels
}

/// Warn that the numbers exclude organizations Sentry refused, so a partial
/// dashboard is not mistaken for a complete one.
pub fn orgs_failed_note(failed: &[String]) -> Panel {
    Panel::Note {
        title: Some(i18n::t("sentry.orgsFailedTitle")),
        message: i18n::tf("sentry.orgsPartial", &[("orgs", &failed.join(", "))]),
    }
}

fn stat_cards(report: &OrgReport, range_label: &str, window: Window) -> Panel {
    let events: u64 = report.issues.iter().map(|i| i.events).sum();
    let new = report
        .issues
        .iter()
        .filter(|i| window.contains(i.first_seen))
        .count();
    let projects: std::collections::HashSet<&str> = report
        .issues
        .iter()
        .filter_map(|i| i.project.as_deref())
        .collect();

    Panel::StatCards {
        title: Some(range_label.to_string()),
        stats: vec![
            Stat {
                label: i18n::t("sentry.stats.issues"),
                value: fmt_count(report.issues.len() as u64),
                sub: None,
            },
            Stat {
                label: i18n::t("sentry.stats.newIssues"),
                value: fmt_count(new as u64),
                sub: None,
            },
            Stat {
                label: i18n::t("sentry.stats.events"),
                value: fmt_count(events),
                sub: None,
            },
            Stat {
                label: i18n::t("sentry.stats.projects"),
                value: projects.len().to_string(),
                sub: None,
            },
        ],
    }
}

/// Where the noise is coming from. Skipped when every issue is in the same
/// project: a one-bar chart says nothing the stat cards did not.
fn events_by_project(report: &OrgReport) -> Option<Panel> {
    let mut totals: HashMap<&str, u64> = HashMap::new();
    for issue in &report.issues {
        // An issue with no project attributes to nothing rather than to a bar
        // labelled "-", which reads as a real project called "-".
        if let Some(project) = issue.project.as_deref() {
            *totals.entry(project).or_insert(0) += issue.events;
        }
    }
    if totals.len() < 2 {
        return None;
    }

    let mut rows: Vec<(&str, u64)> = totals.into_iter().collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let max = rows.first().map(|r| r.1).unwrap_or(0);

    let bars = rows
        .into_iter()
        .take(TOP_PROJECTS)
        .map(|(project, events)| Bar {
            label: project.to_string(),
            // An all-zero range would divide by zero; an empty bar is the
            // honest rendering of "no events".
            value: if max == 0 {
                0.0
            } else {
                events as f64 / max as f64
            },
            display: Some(fmt_count(events)),
        })
        .collect();

    Some(Panel::BarList {
        title: Some(i18n::t("sentry.table.byProject")),
        bars,
    })
}

/// The issue table, or `None` when the range holds no unresolved issues - the
/// caller renders a note instead, since an empty grid reads as a broken fetch
/// rather than a quiet day.
fn issue_table(report: &OrgReport, range: &DateRange, window: Window) -> Option<Panel> {
    if report.issues.is_empty() {
        return None;
    }

    // A single day needs only the clock; a wider range would be unreadable
    // without the date, so the column carries it.
    let time_format = if range.is_single_day() {
        "%H:%M"
    } else {
        "%b %-d, %H:%M"
    };

    let rows = report
        .issues
        .iter()
        .map(|issue| {
            let title = match &issue.permalink {
                Some(url) => link(issue.title.clone(), url.clone()),
                None => text(issue.title.clone()),
            };
            // "Jul 24, 14:30" only sorts right as an instant.
            let last_seen = match issue.last_seen {
                Some(at) => keyed(
                    at.with_timezone(&window.tz).format(time_format).to_string(),
                    at.timestamp() as f64,
                ),
                None => text("-".into()),
            };
            vec![
                title,
                text(issue.culprit.clone().unwrap_or_else(|| "-".into())),
                text(issue.project.clone().unwrap_or_else(|| "-".into())),
                text(level_label(issue.level.as_deref())),
                num(issue.events),
                num(issue.users),
                last_seen,
            ]
        })
        .collect();

    Some(Panel::Table(TableSpec {
        title: Some(i18n::tf(
            "sentry.table.issues",
            &[("range", &range.label())],
        )),
        columns: vec![
            col("issue", i18n::t("sentry.column.issue"), false),
            col("where", i18n::t("sentry.column.where"), false)
                .with_hint(i18n::t("sentry.column.whereHint")),
            col("project", i18n::t("sentry.column.project"), false),
            col("level", i18n::t("sentry.column.level"), false),
            col("events", i18n::t("sentry.column.events"), true)
                .with_hint(i18n::t("sentry.column.eventsHint")),
            col("users", i18n::t("sentry.column.users"), true)
                .with_hint(i18n::t("sentry.column.usersHint")),
            col("lastSeen", i18n::t("sentry.column.lastSeen"), false)
                .with_hint(i18n::t("sentry.column.lastSeenHint")),
        ],
        rows,
    }))
}

/// Sentry's level names are already the words people use; translate the ones
/// worth translating and pass anything unrecognized through rather than
/// blanking a level a newer Sentry introduced.
fn level_label(level: Option<&str>) -> String {
    let Some(level) = level else {
        return "-".to_string();
    };
    let key = format!("sentry.level.{}", level.to_ascii_lowercase());
    let translated = i18n::t(&key);
    if translated == key {
        level.to_string()
    } else {
        translated
    }
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
        text: fmt_count(n),
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
    use chrono::NaiveDate;

    fn ts(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    /// 2026-08-03 IST, the range the fixtures below sit in.
    fn window() -> Window {
        Window {
            start: ts("2026-08-03T00:00:00+05:30"),
            end: ts("2026-08-03T23:59:59+05:30"),
            tz: range::ist(),
        }
    }

    fn today() -> DateRange {
        DateRange {
            start: day(2026, 8, 3),
            end: day(2026, 8, 3),
        }
    }

    fn issue(project: &str, events: u64, users: u64, first_seen: &str) -> Issue {
        Issue {
            title: format!("TypeError in {project}"),
            culprit: Some(format!("app/{project}/index")),
            permalink: Some(format!("https://acme.sentry.io/issues/{project}/")),
            level: Some("error".into()),
            project: Some(project.into()),
            events,
            users,
            first_seen: Some(ts(first_seen)),
            last_seen: Some(ts("2026-08-03T14:30:00+05:30")),
        }
    }

    fn report(issues: Vec<Issue>) -> OrgReport {
        OrgReport {
            account: "work".into(),
            org: "acme".into(),
            issues,
            truncated: false,
        }
    }

    fn stats(panel: &Panel) -> Vec<(String, String)> {
        let Panel::StatCards { stats, .. } = panel else {
            panic!("expected stat cards")
        };
        stats
            .iter()
            .map(|s| (s.label.clone(), s.value.clone()))
            .collect()
    }

    /// "New" is issues first seen inside the range, not every issue with events
    /// in it - an old issue still firing is the opposite of news.
    #[test]
    fn new_counts_only_issues_first_seen_in_the_range() {
        let report = report(vec![
            issue("frontend", 10, 3, "2026-08-03T09:00:00+05:30"),
            // Long-running, still firing today.
            issue("api", 5, 1, "2026-01-14T09:00:00+05:30"),
            // Started just after the window closed.
            issue("worker", 2, 1, "2026-08-04T00:30:00+05:30"),
        ]);

        let cards = stat_cards(&report, "Today", window());
        let values: Vec<String> = stats(&cards).into_iter().map(|(_, v)| v).collect();
        assert_eq!(values[0], "3", "issues");
        assert_eq!(values[1], "1", "new issues");
        assert_eq!(values[2], "17", "events");
        assert_eq!(values[3], "3", "projects");
    }

    /// The breakdown only earns its space once more than one project is
    /// implicated, and it ranks by events rather than by issue count.
    #[test]
    fn project_breakdown_appears_only_when_it_says_something() {
        let single = report(vec![issue("frontend", 10, 3, "2026-08-03T09:00:00+05:30")]);
        assert!(events_by_project(&single).is_none());

        let many = report(vec![
            issue("frontend", 10, 3, "2026-08-03T09:00:00+05:30"),
            issue("api", 40, 9, "2026-08-03T09:00:00+05:30"),
        ]);
        let Some(Panel::BarList { bars, .. }) = events_by_project(&many) else {
            panic!("expected a bar list")
        };
        assert_eq!(bars[0].label, "api");
        assert_eq!(bars[0].display.as_deref(), Some("40"));
        assert_eq!(bars[0].value, 1.0);
        assert_eq!(bars[1].label, "frontend");
        assert_eq!(bars[1].value, 0.25);
    }

    /// An issue Sentry gave no project for must not become a project of its
    /// own: it would render as a bar labelled "-" and inflate the "Projects
    /// affected" count with something that is not a project.
    #[test]
    fn an_issue_without_a_project_is_left_out_of_the_rollups() {
        let mut orphan = issue("frontend", 4, 1, "2026-08-03T09:00:00+05:30");
        orphan.project = None;
        let report = report(vec![
            issue("frontend", 10, 3, "2026-08-03T09:00:00+05:30"),
            issue("api", 40, 9, "2026-08-03T09:00:00+05:30"),
            orphan,
        ]);

        let values: Vec<String> = stats(&stat_cards(&report, "Today", window()))
            .into_iter()
            .map(|(_, v)| v)
            .collect();
        assert_eq!(values[2], "54", "its events still count toward the total");
        assert_eq!(values[3], "2", "but it is not a third project");

        let Some(Panel::BarList { bars, .. }) = events_by_project(&report) else {
            panic!("expected a bar list")
        };
        assert_eq!(bars.len(), 2);
        assert!(!bars.iter().any(|b| b.label == "-"), "{bars:#?}");

        // It is still a row in the table, where "-" reads as "not recorded".
        let Some(Panel::Table(spec)) = issue_table(&report, &today(), window()) else {
            panic!("expected a table")
        };
        assert_eq!(spec.rows.len(), 3);
        assert_eq!(spec.rows[2][2].text, "-");
    }

    /// An empty range renders a note saying so; an empty grid reads as a
    /// broken fetch rather than a quiet day.
    #[test]
    fn an_empty_range_becomes_a_note_not_a_bare_table() {
        let panels = build_panels(&[report(vec![])], &today(), window());
        assert!(
            panels.iter().any(|p| matches!(p, Panel::Note { .. })),
            "expected a note: {panels:#?}"
        );
        assert!(
            !panels.iter().any(|p| matches!(p, Panel::Table(_))),
            "expected no table: {panels:#?}"
        );
    }

    /// One organization needs no heading above the dashboard that already names
    /// the connector; several do, and the account badge only appears when more
    /// than one connection is in play.
    #[test]
    fn headings_appear_only_when_there_is_more_than_one_section() {
        let one = build_panels(&[report(vec![])], &today(), window());
        assert!(!one.iter().any(|p| matches!(p, Panel::Heading { .. })));

        let mut second = report(vec![]);
        second.org = "other".into();
        let two = build_panels(&[report(vec![]), second.clone()], &today(), window());
        let headings: Vec<&Panel> = two
            .iter()
            .filter(|p| matches!(p, Panel::Heading { .. }))
            .collect();
        assert_eq!(headings.len(), 2);
        assert!(
            matches!(headings[0], Panel::Heading { badge: None, .. }),
            "one account needs no badge"
        );

        second.account = "personal".into();
        let mixed = build_panels(&[report(vec![]), second], &today(), window());
        let badged = mixed
            .iter()
            .any(|p| matches!(p, Panel::Heading { badge: Some(b), .. } if b == "personal"));
        assert!(badged, "two accounts must be told apart: {mixed:#?}");
    }

    /// Columns the frontend cannot sort from their text - the formatted counts
    /// and the timestamp - must carry an explicit key, or clicking the header
    /// would order them alphabetically ("1,000" before "9").
    #[test]
    fn unsortable_text_columns_carry_a_sort_key() {
        let report = report(vec![issue(
            "frontend",
            1234,
            7,
            "2026-08-03T09:00:00+05:30",
        )]);
        let Some(Panel::Table(spec)) = issue_table(&report, &today(), window()) else {
            panic!("expected a table")
        };
        let row = &spec.rows[0];

        assert_eq!(row[4].text, "1,234");
        assert_eq!(row[4].sort, Some(1234.0), "events sort numerically");
        assert_eq!(row[6].text, "14:30", "a single day needs only the clock");
        assert_eq!(
            row[6].sort,
            Some(ts("2026-08-03T14:30:00+05:30").timestamp() as f64),
            "last seen sorts as an instant"
        );
        assert_eq!(
            row[0].href.as_deref(),
            Some("https://acme.sentry.io/issues/frontend/")
        );
    }

    /// Across days the row is ambiguous without its date.
    #[test]
    fn the_time_column_carries_the_date_for_multi_day_ranges() {
        let week = DateRange {
            start: day(2026, 7, 28),
            end: day(2026, 8, 3),
        };
        let report = report(vec![issue("frontend", 1, 1, "2026-08-03T09:00:00+05:30")]);
        let Some(Panel::Table(spec)) = issue_table(&report, &week, window()) else {
            panic!("expected a table")
        };
        assert_eq!(spec.rows[0][6].text, "Aug 3, 14:30");
    }

    /// A level fastdash has no wording for must still render as Sentry's own
    /// word rather than as the untranslated catalog key.
    #[test]
    fn unknown_levels_pass_through_instead_of_leaking_the_key() {
        assert_eq!(level_label(Some("error")), i18n::t("sentry.level.error"));
        assert_eq!(level_label(Some("unheard-of")), "unheard-of");
        assert_eq!(level_label(None), "-");
    }

    /// `i18n::t` returns the key itself when a string is missing, so a typo
    /// ships silently as `sentry.empty` in the user's panel.
    #[test]
    fn panel_copy_resolves() {
        for key in [
            "sentry.empty",
            "sentry.truncatedTitle",
            "sentry.orgsFailedTitle",
            "sentry.stats.issues",
            "sentry.stats.newIssues",
            "sentry.stats.events",
            "sentry.stats.projects",
            "sentry.table.byProject",
            "sentry.column.issue",
            "sentry.column.whereHint",
            "sentry.column.eventsHint",
            "sentry.column.usersHint",
            "sentry.column.lastSeenHint",
        ] {
            assert_ne!(i18n::t(key), key, "missing catalog entry: {key}");
        }

        let issues = i18n::tf("sentry.table.issues", &[("range", "Today")]);
        assert!(issues.contains("Today"), "{issues}");
        assert!(!issues.contains("{range}"), "{issues}");

        let partial = i18n::tf("sentry.orgsPartial", &[("orgs", "acme")]);
        assert!(partial.contains("acme"), "{partial}");
        assert!(!partial.contains("{orgs}"), "{partial}");
    }
}
