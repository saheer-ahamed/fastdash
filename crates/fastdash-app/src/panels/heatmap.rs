//! A GitHub-style year grid: one column per week (Sunday first, as GitHub lays
//! it out), one row per weekday, month labels above and a year rail on the
//! right. The connector supplies each day's shade level, so this only paints.

use chrono::{Datelike, NaiveDate};
use fastdash_core::engine::panel::{HeatDay, HeatmapYear};
use gpui::{div, prelude::*, px, AnyElement, App, ElementId, Hsla, SharedString, Window};

use crate::i18n::t;
use crate::theme::Theme;
use crate::ui::{self, tooltip, AxisScroll};

const CELL: f32 = 11.;
const GAP: f32 = 3.;
/// Weekday rows that get a label (Mon / Wed / Fri, like GitHub).
const LABELLED_ROWS: [usize; 3] = [1, 3, 5];

struct HeatmapState {
    year: Option<String>,
}

struct Placed<'a> {
    day: &'a HeatDay,
    col: usize,
    row: usize,
}

struct Layout<'a> {
    cells: Vec<Placed<'a>>,
    columns: usize,
    months: Vec<(usize, String)>,
}

fn parse(day: &HeatDay) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(&day.date, "%Y-%m-%d").ok()
}

/// Place every day on the (week, weekday) grid and derive the month labels. The
/// grid starts on the Sunday of the first day's week, so a window that begins
/// mid-week leaves the leading cells of column 0 empty, as GitHub does.
fn layout(days: &[HeatDay]) -> Layout<'_> {
    let Some(first) = days.first().and_then(parse) else {
        return Layout {
            cells: vec![],
            columns: 0,
            months: vec![],
        };
    };
    let start = first - chrono::Duration::days(first.weekday().num_days_from_sunday() as i64);
    let cells: Vec<Placed> = days
        .iter()
        .filter_map(|day| {
            let index = (parse(day)? - start).num_days().max(0) as usize;
            Some(Placed {
                day,
                col: index / 7,
                row: index % 7,
            })
        })
        .collect();
    let columns = cells.last().map_or(0, |c| c.col + 1);

    // Label a column when its first day starts a new month; drop labels that
    // would sit on top of the next one (the leading partial month).
    let mut months: Vec<(usize, String)> = Vec::new();
    let mut seen = None;
    for cell in &cells {
        let Some(date) = parse(cell.day) else {
            continue;
        };
        if seen != Some(date.month()) {
            seen = Some(date.month());
            if months.last().map(|(c, _)| *c) != Some(cell.col) {
                months.push((cell.col, date.format("%b").to_string()));
            }
        }
    }
    let spaced = months
        .iter()
        .enumerate()
        .filter(|(i, (col, _))| *i + 1 == months.len() || months[i + 1].0 - col >= 3)
        .map(|(_, m)| m.clone())
        .collect();

    Layout {
        cells,
        columns,
        months: spaced,
    }
}

/// Empty days sit just above the card, so an idle year still reads as a grid;
/// each level then steps clearly toward the full fill colour.
fn shade(level: u8, t: &Theme) -> Option<Hsla> {
    match level {
        0 => None,
        1 => Some(t.fill_mix(0.28)),
        2 => Some(t.fill_mix(0.50)),
        3 => Some(t.fill_mix(0.74)),
        _ => Some(t.fill),
    }
}

fn swatch(level: u8, t: &Theme) -> gpui::Div {
    let cell = div().flex_none().size(px(CELL));
    match shade(level, t) {
        Some(color) => cell.bg(color),
        None => cell.bg(t.panel_2).border_1().border_color(t.border),
    }
}

pub fn render(
    title: Option<&str>,
    years: &[HeatmapYear],
    id: ElementId,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let state = window.use_keyed_state(id.clone(), cx, |_, _| HeatmapState { year: None });
    let selected = state.read(cx).year.clone();
    let Some(active) = years
        .iter()
        .find(|y| Some(&y.label) == selected.as_ref())
        .or_else(|| years.first())
    else {
        return div().into_any_element();
    };
    let th = ui::theme(cx);
    let grid = layout(&active.days);
    let step = CELL + GAP;
    let width = grid.columns as f32 * step - GAP;

    let months = div()
        .relative()
        .h(px(12.))
        .w(px(width.max(0.)))
        .text_size(px(10.))
        .line_height(px(12.))
        .text_color(th.muted)
        .children(grid.months.iter().map(|(col, label)| {
            div()
                .absolute()
                .left(px(*col as f32 * step))
                .bottom_0()
                .whitespace_nowrap()
                .child(label.clone())
        }));

    let weekdays = div()
        .relative()
        .w(px(24.))
        .h(px(7. * step - GAP))
        .text_size(px(10.))
        .line_height(px(CELL))
        .text_color(th.muted)
        .children(LABELLED_ROWS.iter().map(|row| {
            // 2026-07-26 is a Sunday; offset from it to name the row.
            let name = NaiveDate::from_ymd_opt(2026, 7, 26 + *row as u32)
                .map(|d| d.format("%a").to_string())
                .unwrap_or_default();
            div()
                .absolute()
                .right_0()
                .top(px(*row as f32 * step))
                .child(name)
        }));

    let mut columns: Vec<Vec<Option<&HeatDay>>> = vec![vec![None; 7]; grid.columns];
    for cell in &grid.cells {
        columns[cell.col][cell.row] = Some(cell.day);
    }
    let cells = div()
        .flex()
        .gap(px(GAP))
        .children(columns.into_iter().enumerate().map(|(ci, week)| {
            div()
                .flex()
                .flex_col()
                .gap(px(GAP))
                .children(week.into_iter().enumerate().map(move |(ri, day)| {
                    match day {
                        Some(day) => swatch(day.level, &th)
                            .id(("day", ci * 7 + ri))
                            .tooltip(tooltip::text(day.tooltip.clone()))
                            .into_any_element(),
                        None => div().flex_none().size(px(CELL)).into_any_element(),
                    }
                }))
        }));

    let chart = div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(
            div()
                .flex()
                .gap(px(6.))
                .child(div().w(px(24.)))
                .child(months),
        )
        .child(div().flex().gap(px(6.)).child(weekdays).child(cells));

    let legend = div()
        .flex()
        .items_center()
        .justify_end()
        .gap(px(GAP))
        .mt(px(10.))
        .text_size(px(11.))
        .text_color(th.muted)
        .child(div().mr(px(3.)).child(t("heatmap.less")))
        .children((0..=4).map(|l| swatch(l, &th)))
        .child(div().ml(px(3.)).child(t("heatmap.more")));

    let main = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .child(
            div()
                .mb(px(10.))
                .text_color(th.bright)
                .child(active.summary.clone()),
        )
        .child(div().id(id.clone()).scroll_x().pb(px(2.)).child(chart))
        .child(legend);

    let rail = (years.len() > 1).then(|| {
        div()
            .flex()
            .flex_col()
            .flex_none()
            .gap(px(2.))
            .children(years.iter().map(|y| {
                let is_active = y.label == active.label;
                let label: SharedString = y.label.clone().into();
                let base = div()
                    .id(SharedString::from(format!("year-{}", y.label)))
                    .px(px(12.))
                    .py(px(4.))
                    .border_1()
                    .text_size(px(12.))
                    .cursor_pointer()
                    .child(label);
                let base = if is_active {
                    base.bg(th.panel_2)
                        .border_color(th.border_bright)
                        .text_color(th.bright)
                } else {
                    base.border_color(gpui::transparent_black())
                        .text_color(th.text)
                        .hover(move |s| s.border_color(th.border).text_color(th.bright))
                };
                let state = state.clone();
                let year = y.label.clone();
                base.on_click(move |_, _, cx| {
                    state.update(cx, |s, cx| {
                        s.year = Some(year.clone());
                        cx.notify();
                    })
                })
            }))
    });

    ui::card(cx)
        .children(title.map(|title| ui::card_title(title.to_string(), cx)))
        .child(
            div()
                .flex()
                .items_start()
                .gap(px(14.))
                .child(main)
                .children(rail),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(date: &str) -> HeatDay {
        HeatDay {
            date: date.into(),
            level: 0,
            tooltip: String::new(),
        }
    }

    #[test]
    fn a_mid_week_start_leaves_the_first_column_partly_empty() {
        // 2026-07-29 is a Wednesday: row 3 of column 0.
        let days = [day("2026-07-29"), day("2026-07-30"), day("2026-08-02")];
        let grid = layout(&days);
        assert_eq!((grid.cells[0].col, grid.cells[0].row), (0, 3));
        // The following Sunday opens column 1.
        assert_eq!((grid.cells[2].col, grid.cells[2].row), (1, 0));
        assert_eq!(grid.columns, 2);
    }

    #[test]
    fn crowded_month_labels_are_dropped() {
        // 2026-10-31 is a Saturday, so October owns only column 0 and
        // November opens column 1: October's label would sit on November's.
        let days: Vec<HeatDay> = (0..40)
            .map(|i| {
                let d = NaiveDate::from_ymd_opt(2026, 10, 28).unwrap() + chrono::Duration::days(i);
                day(&d.format("%Y-%m-%d").to_string())
            })
            .collect();
        let grid = layout(&days);
        let labels: Vec<&str> = grid.months.iter().map(|(_, l)| l.as_str()).collect();
        assert_eq!(labels, vec!["Nov", "Dec"]);
    }
}
