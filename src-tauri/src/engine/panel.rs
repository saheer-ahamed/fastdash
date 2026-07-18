//! Generic render primitives that cross the Rust -> JS boundary.
//!
//! Connectors emit `Panel`s; the frontend is a single dumb renderer over these
//! shapes and never learns what "GitHub" or "Slack" is. Adding a connector that
//! reuses these primitives needs no frontend work.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Panel {
    /// A row of KPI tiles (tokens, cost, mentions today, ...).
    StatCards {
        title: Option<String>,
        stats: Vec<Stat>,
    },
    /// A progress bar with an optional limit (5h window, weekly usage).
    Meter {
        label: String,
        used: f64,
        limit: Option<f64>,
        unit: String,
        /// Pre-formatted caption, e.g. "62% - resets in 1h14m".
        caption: Option<String>,
    },
    /// A sortable table (PR counts, per-model tokens, line contributions).
    Table(TableSpec),
    /// A horizontal bar list (effort split, top contributors).
    BarList {
        title: Option<String>,
        bars: Vec<Bar>,
    },
    /// A vertical list of links (Slack mentions, PR list).
    List {
        title: Option<String>,
        items: Vec<ListItem>,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stat {
    pub label: String,
    pub value: String,
    pub sub: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bar {
    pub label: String,
    /// Fraction 0.0..=1.0 used for the bar width.
    pub value: f64,
    /// Optional pre-formatted value shown at the end of the bar.
    pub display: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableSpec {
    pub title: Option<String>,
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<Cell>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Column {
    pub key: String,
    pub label: String,
    /// Right-align and sort numerically when true.
    pub numeric: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Cell {
    pub text: String,
    pub href: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListItem {
    pub title: String,
    pub subtitle: Option<String>,
    pub meta: Option<String>,
    pub href: Option<String>,
}
