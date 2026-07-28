//! Generic render primitives that cross the Rust -> JS boundary.
//!
//! Connectors emit `Panel`s; the frontend is a single dumb renderer over these
//! shapes and never learns what "GitHub" or "Claude" is. Adding a connector that
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
    /// A section heading with an optional badge (e.g. the plan name).
    Heading {
        title: String,
        badge: Option<String>,
    },
    /// A progress bar with an optional limit (5h window, weekly usage).
    Meter {
        label: String,
        used: f64,
        limit: Option<f64>,
        unit: String,
        /// Secondary line under the label, e.g. "Resets in 47 min".
        sub: Option<String>,
        /// Right-aligned value, e.g. "31% used".
        caption: Option<String>,
    },
    /// A sortable table (PR counts, per-model tokens, line contributions).
    Table(TableSpec),
    /// A horizontal bar list (effort split, top contributors).
    BarList {
        title: Option<String>,
        bars: Vec<Bar>,
    },
    /// A vertical list of links (e.g. a PR list).
    List {
        title: Option<String>,
        items: Vec<ListItem>,
    },
    /// A short explanatory card, e.g. why a panel can't be shown yet.
    Note {
        title: Option<String>,
        message: String,
    },
    /// A GitHub-style year grid of daily activity, with a year selector.
    Heatmap {
        title: Option<String>,
        /// Most recent year first; the frontend selects the first by default.
        years: Vec<HeatmapYear>,
    },
}

/// One selectable year of a [`Panel::Heatmap`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeatmapYear {
    /// Year-rail label, e.g. "2026".
    pub label: String,
    /// Headline above the grid, e.g. "104 contributions in the last year".
    pub summary: String,
    /// Every day of the window, ascending and contiguous (zero days included).
    pub days: Vec<HeatDay>,
}

/// A single day cell. `level` is the shade bucket, so each connector decides
/// how its own unit maps onto the five shades; the frontend only paints.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeatDay {
    /// ISO `YYYY-MM-DD`.
    pub date: String,
    /// Shade bucket, 0 (empty) ..= 4 (most active).
    pub level: u8,
    /// Hover text, e.g. "5 contributions on Jul 24, 2026".
    pub tooltip: String,
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
    /// One sentence saying exactly what the column counts, shown on hover.
    /// A header label has to stay short; this is where the precision goes.
    pub hint: Option<String>,
}

impl Column {
    pub fn new(key: &str, label: impl Into<String>, numeric: bool) -> Self {
        Column {
            key: key.into(),
            label: label.into(),
            numeric,
            hint: None,
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Cell {
    pub text: String,
    pub href: Option<String>,
    /// Numeric key to sort this cell by when its display text would sort wrong
    /// on its own - `"1.2M"`, `"$3.40"`, `"+10 / -2"`, `"Jul 24, 14:30"`.
    /// `None` means "sort by `text`"; in a column where some cells carry a key,
    /// the ones without sink to the bottom of either direction.
    pub sort: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListItem {
    pub title: String,
    pub subtitle: Option<String>,
    pub meta: Option<String>,
    pub href: Option<String>,
}
