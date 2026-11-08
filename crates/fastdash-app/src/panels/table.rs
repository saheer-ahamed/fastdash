//! A table that sorts on any column and pages once it passes [`PAGE_SIZE`]
//! rows.
//!
//! Sorting is off by default: connectors order their rows meaningfully (most
//! merged first, and so on), and a third click on a header restores that.
//! Columns size to their content - the font is monospace, so a column's width
//! is its longest cell in characters - and share any spare width in proportion,
//! the way an auto-layout HTML table at `width: 100%` does. A table wider than
//! its card scrolls sideways instead of squeezing.

use std::cmp::Ordering;

use fastdash_core::engine::panel::{Cell, Column, TableSpec};
use gpui::{
    div, font, prelude::*, px, AnyElement, App, Context, ElementId, FontWeight, Pixels,
    SharedString, Window,
};

use crate::i18n::{t, tf};
use crate::ui::{self, tooltip};

pub const PAGE_SIZE: usize = 15;
const FONT_SIZE: f32 = 12.5;
/// Horizontal padding inside a cell, both sides together.
const CELL_PAD: f32 = 20.;
/// The sort caret and the gap before it.
const CARET: f32 = 5.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dir {
    Asc,
    Desc,
}

#[derive(Default)]
struct TableState {
    page: usize,
    sort: Option<(usize, Dir)>,
}

/// The direction a column starts in: counts read best largest-first, names A-Z.
fn first_dir(numeric: bool) -> Dir {
    if numeric {
        Dir::Desc
    } else {
        Dir::Asc
    }
}

/// asc -> desc -> unsorted, starting in whichever direction reads best.
fn next_sort(current: Option<(usize, Dir)>, col: usize, numeric: bool) -> Option<(usize, Dir)> {
    let start = first_dir(numeric);
    match current {
        Some((c, d)) if c == col && d == start => Some((
            col,
            if start == Dir::Asc {
                Dir::Desc
            } else {
                Dir::Asc
            },
        )),
        Some((c, _)) if c == col => None,
        _ => Some((col, start)),
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Value {
    Num(f64),
    Text(String),
}

/// What a cell compares as: its explicit sort key when the connector supplied
/// one ("1.2M" and "Jul 24, 14:30" sort wrong as text), the number its text
/// parses to in a numeric column, otherwise the text. `None` means "no value";
/// those rows sink to the bottom in either direction.
fn cell_value(cell: Option<&Cell>, numeric: bool) -> Option<Value> {
    let cell = cell?;
    if let Some(key) = cell.sort {
        return Some(Value::Num(key));
    }
    let text = cell.text.trim();
    if text.is_empty() {
        return None;
    }
    if numeric {
        let digits: String = text
            .chars()
            .filter(|c| c.is_ascii_digit() || matches!(c, '.' | '+' | '-'))
            .collect();
        return digits.parse().ok().map(Value::Num);
    }
    Some(Value::Text(text.to_string()))
}

/// Case-insensitive natural order: digit runs compare as numbers, so "pr-9"
/// sorts before "pr-10".
fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut a = a.chars().peekable();
    let mut b = b.chars().peekable();
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let mut na = String::new();
                while let Some(c) = a.peek().copied().filter(char::is_ascii_digit) {
                    na.push(c);
                    a.next();
                }
                let mut nb = String::new();
                while let Some(c) = b.peek().copied().filter(char::is_ascii_digit) {
                    nb.push(c);
                    b.next();
                }
                let na = na.trim_start_matches('0');
                let nb = nb.trim_start_matches('0');
                let ord = na.len().cmp(&nb.len()).then_with(|| na.cmp(nb));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(x), Some(y)) => {
                let ord = x.to_lowercase().cmp(y.to_lowercase());
                if ord != Ordering::Equal {
                    return ord;
                }
                a.next();
                b.next();
            }
        }
    }
}

fn compare(a: &Value, b: &Value) -> Ordering {
    match (a, b) {
        (Value::Num(x), Value::Num(y)) => x.partial_cmp(y).unwrap_or(Ordering::Equal),
        (Value::Num(x), Value::Text(y)) => natural_cmp(&x.to_string(), y),
        (Value::Text(x), Value::Num(y)) => natural_cmp(x, &y.to_string()),
        (Value::Text(x), Value::Text(y)) => natural_cmp(x, y),
    }
}

/// Row indices in display order. Ties keep the connector's order.
fn sorted_rows(spec: &TableSpec, sort: Option<(usize, Dir)>) -> Vec<usize> {
    let mut order: Vec<usize> = (0..spec.rows.len()).collect();
    let Some((col, dir)) = sort.filter(|(c, _)| *c < spec.columns.len()) else {
        return order;
    };
    let numeric = spec.columns[col].numeric;
    let values: Vec<Option<Value>> = spec
        .rows
        .iter()
        .map(|row| cell_value(row.get(col), numeric))
        .collect();
    order.sort_by(|&a, &b| {
        let ord = match (&values[a], &values[b]) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (Some(x), Some(y)) => {
                let ord = compare(x, y);
                if dir == Dir::Asc {
                    ord
                } else {
                    ord.reverse()
                }
            }
        };
        ord.then(a.cmp(&b))
    });
    order
}

/// Each column's natural width: its widest cell or header, in characters.
fn column_chars(spec: &TableSpec) -> Vec<usize> {
    spec.columns
        .iter()
        .enumerate()
        .map(|(i, col)| {
            let cells = spec
                .rows
                .iter()
                .filter_map(|row| row.get(i))
                .map(|c| c.text.chars().count())
                .max()
                .unwrap_or(0);
            cells.max(col.label.chars().count() + 1)
        })
        .collect()
}

pub fn render(spec: &TableSpec, id: ElementId, window: &mut Window, cx: &mut App) -> AnyElement {
    let state = window.use_keyed_state(id.clone(), cx, |_, _| TableState::default());
    let (page, sort) = {
        let s = state.read(cx);
        (s.page, s.sort)
    };
    let th = ui::theme(cx);

    let ch = char_width(window, cx);
    let widths: Vec<Pixels> = column_chars(spec)
        .into_iter()
        .map(|n| ch * n as f32 + px(CELL_PAD + CARET))
        .collect();
    let total: Pixels = widths.iter().copied().fold(px(0.), |a, b| a + b);

    let order = sorted_rows(spec, sort);
    let rows_total = order.len();
    let pages = rows_total.div_ceil(PAGE_SIZE).max(1);
    let page = page.min(pages - 1);
    let start = page * PAGE_SIZE;
    let paginated = rows_total > PAGE_SIZE;
    let visible: Vec<usize> = if paginated {
        order.iter().copied().skip(start).take(PAGE_SIZE).collect()
    } else {
        order
    };

    let col_cell = |i: usize, numeric: bool| {
        let w = widths[i];
        div()
            .flex()
            .flex_basis(w)
            .min_w(w)
            .map(|mut d| {
                // Spare width is shared in proportion to natural width.
                d.style().flex_grow = Some(f32::from(w));
                d
            })
            .when(numeric, |d| d.justify_end())
            .px(px(CELL_PAD / 2.))
            .py(px(6.))
            .whitespace_nowrap()
    };

    let header = div()
        .flex()
        .min_w(total)
        .border_b_1()
        .border_color(th.border_bright)
        .children(spec.columns.iter().enumerate().map(|(ci, col)| {
            header_cell(col, ci, sort, col_cell(ci, col.numeric), state.clone(), cx)
        }));

    let count = visible.len();
    let body = visible.into_iter().enumerate().map(|(vi, ri)| {
        let row = &spec.rows[ri];
        div()
            .id(("row", ri))
            .flex()
            .min_w(total)
            .when(vi + 1 < count, |d| d.border_b_1().border_color(th.border))
            .hover(move |s| s.bg(th.panel_2))
            .children(row.iter().enumerate().map(|(ci, cell)| {
                let numeric = spec.columns.get(ci).is_some_and(|c| c.numeric);
                let content = match &cell.href {
                    Some(href) => super::link(
                        ("cell", ri * 1000 + ci),
                        cell.text.clone(),
                        href.clone(),
                        cx,
                    )
                    .into_any_element(),
                    None => div().child(cell.text.clone()).into_any_element(),
                };
                col_cell(ci, numeric).child(content)
            }))
    });

    ui::card(cx)
        .children(spec.title.clone().map(|title| ui::card_title(title, cx)))
        .child(
            div()
                .id(id)
                .overflow_x_scroll()
                .text_size(px(FONT_SIZE))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w(total)
                        .child(header)
                        .children(body),
                ),
        )
        .when(paginated, |card| {
            let end = (start + PAGE_SIZE).min(rows_total);
            card.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(14.))
                    .mt(px(10.))
                    .pt(px(10.))
                    .border_t_1()
                    .border_color(th.border)
                    .child(pager_button(
                        "prev",
                        t("pager.prev"),
                        page > 0,
                        {
                            let state = state.clone();
                            move |cx| {
                                state.update(cx, |s, cx| set_page(s, page.saturating_sub(1), cx))
                            }
                        },
                        cx,
                    ))
                    .child(div().text_color(th.muted).child(tf(
                        "pager.range",
                        &[
                            ("start", &(start + 1).to_string()),
                            ("end", &end.to_string()),
                            ("total", &rows_total.to_string()),
                        ],
                    )))
                    .child(pager_button(
                        "next",
                        t("pager.next"),
                        page + 1 < pages,
                        {
                            let state = state.clone();
                            move |cx| state.update(cx, |s, cx| set_page(s, page + 1, cx))
                        },
                        cx,
                    )),
            )
        })
        .into_any_element()
}

fn set_page(state: &mut TableState, page: usize, cx: &mut Context<TableState>) {
    state.page = page;
    cx.notify();
}

fn header_cell(
    col: &Column,
    ci: usize,
    sort: Option<(usize, Dir)>,
    cell: gpui::Div,
    state: gpui::Entity<TableState>,
    cx: &App,
) -> AnyElement {
    let th = ui::theme(cx);
    let active = sort.filter(|(c, _)| *c == ci).map(|(_, d)| d);
    let caret = match active {
        Some(Dir::Asc) => "↑",
        Some(Dir::Desc) => "↓",
        None => "↕",
    };
    let group: SharedString = format!("th-{ci}").into();
    let label = div()
        .child(SharedString::from(col.label.to_uppercase()))
        .when(col.hint.is_some(), |d| {
            // An underline that says "hover me for what this counts".
            d.underline().text_decoration_color(th.border_bright)
        });
    let numeric = col.numeric;
    let hint: SharedString = col
        .hint
        .clone()
        .unwrap_or_else(|| tf("table.sortBy", &[("column", &col.label)]).to_string())
        .into();
    cell.id(("th", ci))
        .group(group.clone())
        .gap(px(5.))
        .items_center()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(if active.is_some() {
            th.bright
        } else {
            th.muted
        })
        .cursor_pointer()
        .hover(move |s| s.text_color(th.bright))
        .child(label)
        .child(
            div()
                .text_size(px(10.))
                .opacity(if active.is_some() { 1.0 } else { 0.0 })
                .group_hover(group, |s| {
                    s.opacity(if active.is_some() { 1.0 } else { 0.45 })
                })
                .child(caret),
        )
        .tooltip(tooltip::text(hint))
        .on_click(move |_, _, cx| {
            state.update(cx, |s, cx| {
                s.sort = next_sort(s.sort, ci, numeric);
                s.page = 0;
                cx.notify();
            })
        })
        .into_any_element()
}

fn pager_button(
    id: &'static str,
    label: SharedString,
    enabled: bool,
    on_click: impl Fn(&mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let th = ui::theme(cx);
    let base = div().id(id).px(px(4.)).py(px(2.)).child(label);
    if enabled {
        base.text_color(th.text)
            .cursor_pointer()
            .hover(move |s| s.text_color(th.bright))
            .on_click(move |_, _, cx| on_click(cx))
            .into_any_element()
    } else {
        base.text_color(th.muted).opacity(0.4).into_any_element()
    }
}

/// The advance of one character in the table font.
fn char_width(window: &Window, cx: &App) -> Pixels {
    let family = crate::fonts::family(cx);
    let text_system = window.text_system();
    let font_id = text_system.resolve_font(&font(family));
    text_system
        .ch_width(font_id, px(FONT_SIZE))
        .unwrap_or(px(FONT_SIZE * 0.6))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(text: &str, sort: Option<f64>) -> Cell {
        Cell {
            text: text.into(),
            href: None,
            sort,
        }
    }

    fn spec(rows: Vec<Vec<Cell>>, numeric: bool) -> TableSpec {
        TableSpec {
            title: None,
            columns: vec![Column::new("a", "A", numeric)],
            rows,
        }
    }

    #[test]
    fn sorting_cycles_from_the_natural_direction_back_to_unsorted() {
        let s = next_sort(None, 0, true);
        assert_eq!(s, Some((0, Dir::Desc)));
        let s = next_sort(s, 0, true);
        assert_eq!(s, Some((0, Dir::Asc)));
        assert_eq!(next_sort(s, 0, true), None);
        assert_eq!(next_sort(None, 1, false), Some((1, Dir::Asc)));
    }

    #[test]
    fn sort_keys_beat_display_text_and_blanks_sink_both_ways() {
        let rows = vec![
            vec![cell("1.2M", Some(1_200_000.0))],
            vec![cell("", None)],
            vec![cell("900K", Some(900_000.0))],
        ];
        let s = spec(rows, true);
        assert_eq!(sorted_rows(&s, Some((0, Dir::Desc))), vec![0, 2, 1]);
        assert_eq!(sorted_rows(&s, Some((0, Dir::Asc))), vec![2, 0, 1]);
        assert_eq!(sorted_rows(&s, None), vec![0, 1, 2]);
    }

    #[test]
    fn numeric_text_parses_and_text_sorts_naturally() {
        let rows = vec![vec![cell("$3.40", None)], vec![cell("$10.00", None)]];
        assert_eq!(
            sorted_rows(&spec(rows, true), Some((0, Dir::Asc))),
            vec![0, 1]
        );
        let rows = vec![vec![cell("pr-10", None)], vec![cell("PR-9", None)]];
        assert_eq!(
            sorted_rows(&spec(rows, false), Some((0, Dir::Asc))),
            vec![1, 0]
        );
    }

    #[test]
    fn ties_keep_the_connector_order() {
        let rows = vec![
            vec![cell("x", None)],
            vec![cell("X", None)],
            vec![cell("x", None)],
        ];
        assert_eq!(
            sorted_rows(&spec(rows, false), Some((0, Dir::Desc))),
            vec![0, 1, 2]
        );
    }
}
