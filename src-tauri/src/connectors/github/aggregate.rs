//! Per-contributor rollups and `Panel` construction from the fetched PR sets.

use std::collections::HashMap;

use chrono::{DateTime, FixedOffset, Utc};

use crate::engine::i18n;
use crate::engine::panel::{Cell, Column, Panel, Stat, TableSpec};

/// Outcome of a PR within the day's window.
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

/// One row of the "PRs today" list (union of all four search sets).
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
    /// Merged-today PRs, per author, with line counts (from GraphQL).
    pub line_contribs: Vec<LineContrib>,
    /// Union of PRs seen today, for the "PRs today" table.
    pub pr_list: Vec<PrEntry>,
}

/// A merged-today PR's line contribution, attributed to its author.
#[derive(Debug, Clone)]
pub struct LineContrib {
    pub author: String,
    pub additions: u64,
    pub deletions: u64,
}

/// Build the connector's panels: a `StatCards` header plus the three tables.
pub fn build_panels(rollup: &Rollup, ist: FixedOffset) -> Vec<Panel> {
    vec![
        stat_cards(rollup),
        pr_activity_table(rollup),
        line_contributions_table(rollup),
        pr_list_table(rollup, ist),
    ]
}

fn stat_cards(rollup: &Rollup) -> Panel {
    let total_opened: u64 = rollup.opened.values().sum();
    let total_merged: u64 = rollup.merged.values().sum();

    // Anyone who opened, merged, closed, or has an open PR counts as active.
    let mut active: std::collections::HashSet<&String> = std::collections::HashSet::new();
    for map in [&rollup.opened, &rollup.merged, &rollup.closed, &rollup.open] {
        active.extend(map.keys());
    }

    Panel::StatCards {
        title: Some(i18n::t("github.stats.title")),
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

fn pr_activity_table(rollup: &Rollup) -> Panel {
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
        title: Some(i18n::t("github.table.prActivity")),
        columns: vec![
            col("contributor", i18n::t("github.column.contributor"), false),
            col("merged", i18n::t("github.column.merged"), true),
            col("opened", i18n::t("github.column.opened"), true),
            col("closed", i18n::t("github.column.closedNoMerge"), true),
            col("open", i18n::t("github.column.open"), true),
        ],
        rows: table_rows,
    })
}

fn line_contributions_table(rollup: &Rollup) -> Panel {
    // Aggregate the merged-today PRs per author.
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
                text(format_net(net)),
                num(prs),
            ]
        })
        .collect();

    Panel::Table(TableSpec {
        title: Some(i18n::t("github.table.lineContributions")),
        columns: vec![
            col("contributor", i18n::t("github.column.contributor"), false),
            col("total", i18n::t("github.column.total"), true),
            col("additions", i18n::t("github.column.additions"), true),
            col("deletions", i18n::t("github.column.deletions"), true),
            col("net", i18n::t("github.column.net"), true),
            col("prs", i18n::t("github.column.prs"), true),
        ],
        rows: table_rows,
    })
}

fn pr_list_table(rollup: &Rollup, ist: FixedOffset) -> Panel {
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
            let delta = match (pr.additions, pr.deletions) {
                (Some(a), Some(d)) => format!("+{a} / -{d}"),
                _ => "-".into(),
            };
            let time = pr
                .at
                .map(|t| t.with_timezone(&ist).format("%H:%M").to_string())
                .unwrap_or_else(|| "-".into());
            vec![
                text(author),
                text(pr.name_with_owner.clone()),
                link(pr.title.clone(), pr.url.clone()),
                text(pr.state.label()),
                text(delta),
                text(time),
            ]
        })
        .collect();

    Panel::Table(TableSpec {
        title: Some(i18n::t("github.table.prList")),
        columns: vec![
            col("author", i18n::t("github.column.contributor"), false),
            col("repo", i18n::t("github.column.repo"), false),
            col("title", i18n::t("github.column.title"), false),
            col("state", i18n::t("github.column.state"), false),
            col("delta", i18n::t("github.column.delta"), false),
            col("time", i18n::t("github.column.time"), false),
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
    Column {
        key: key.into(),
        label: label.into(),
        numeric,
    }
}

fn text(s: String) -> Cell {
    Cell {
        text: s,
        href: None,
    }
}

fn num(n: u64) -> Cell {
    Cell {
        text: n.to_string(),
        href: None,
    }
}

fn link(text: String, href: String) -> Cell {
    Cell {
        text,
        href: Some(href),
    }
}
