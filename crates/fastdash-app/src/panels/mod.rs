//! The one generic renderer: `Panel` in, element out.
//!
//! It knows nothing about which connector produced a panel - that is what keeps
//! adding a connector a zero-UI-change job. Stateful panels (sortable tables,
//! the heatmap's year rail) keep their state per panel through GPUI's keyed
//! element state, so it survives re-renders but not leaving the view.

mod heatmap;
mod table;

use fastdash_core::engine::connector::{Health, Snapshot};
use fastdash_core::engine::panel::{Panel, Stat};
use gpui::{div, prelude::*, px, AnyElement, App, ElementId, FontWeight, SharedString, Window};

use crate::i18n::t;
use crate::ui::{self, Tone};

/// The status banner for a snapshot, if its health is anything but OK.
///
/// Auth, rate-limit and misconfiguration problems carry copy the user can act
/// on, so everyone sees it. A generic failure shows a plain reassuring line;
/// the raw technical string is developer-mode only.
pub fn status_banner(status: &Health, dev_mode: bool, cx: &App) -> Option<AnyElement> {
    let (tone, message, detail) = match status {
        Health::Ok => return None,
        Health::NeedsAuth { message } | Health::Misconfigured { message } => {
            (Tone::Warn, SharedString::from(message.clone()), None)
        }
        Health::RateLimited { .. } => (Tone::Warn, t("status.rateLimited"), None),
        Health::Error { message } => (
            Tone::Err,
            t("status.error"),
            (dev_mode && !message.is_empty()).then(|| SharedString::from(message.clone())),
        ),
    };
    Some(ui::banner(tone, message, detail, cx).into_any_element())
}

/// Every panel of a snapshot, in order. `scope` names the view the snapshot
/// belongs to (connector + range), so per-panel state never leaks between
/// views that happen to have a panel at the same index.
pub fn render_panels(
    snapshot: &Snapshot,
    scope: &str,
    window: &mut Window,
    cx: &mut App,
) -> Vec<AnyElement> {
    snapshot
        .panels
        .iter()
        .enumerate()
        .map(|(i, panel)| {
            let id = ElementId::Name(format!("{scope}#{i}").into());
            render(panel, id, window, cx)
        })
        .collect()
}

fn render(panel: &Panel, id: ElementId, window: &mut Window, cx: &mut App) -> AnyElement {
    match panel {
        Panel::Heading { title, badge } => {
            ui::section_heading(title.clone(), badge.clone().map(Into::into), cx).into_any_element()
        }
        Panel::StatCards { title, stats } => stat_cards(title.as_deref(), stats, cx),
        Panel::Meter {
            label,
            used,
            limit,
            sub,
            caption,
            ..
        } => meter(label, *used, *limit, sub.as_deref(), caption.as_deref(), cx),
        Panel::Table(spec) => table::render(spec, id, window, cx),
        Panel::BarList { title, bars } => {
            let th = ui::theme(cx);
            ui::card(cx)
                .children(title.clone().map(|title| ui::card_title(title, cx)))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(7.))
                        .children(bars.iter().map(|b| {
                            div()
                                .flex()
                                .items_center()
                                .gap(px(10.))
                                .child(
                                    div()
                                        .w(px(110.))
                                        .flex_none()
                                        .truncate()
                                        .child(b.label.clone()),
                                )
                                .child(div().flex_1().min_w_0().child(ui::track(
                                    b.value as f32,
                                    10.,
                                    cx,
                                )))
                                .child(
                                    div()
                                        .w(px(90.))
                                        .flex_none()
                                        .flex()
                                        .justify_end()
                                        .text_color(th.muted)
                                        .child(b.display.clone().unwrap_or_default()),
                                )
                        })),
                )
                .into_any_element()
        }
        Panel::List { title, items } => {
            let th = ui::theme(cx);
            let count = items.len();
            ui::card(cx)
                .children(title.clone().map(|title| ui::card_title(title, cx)))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .children(items.iter().enumerate().map(|(i, item)| {
                            let main =
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .min_w_0()
                                    .child(match &item.href {
                                        Some(href) => {
                                            link(("item", i), item.title.clone(), href.clone(), cx)
                                                .into_any_element()
                                        }
                                        None => div().child(item.title.clone()).into_any_element(),
                                    })
                                    .children(item.subtitle.clone().map(|s| {
                                        div().text_color(th.muted).child(format!(" {s}"))
                                    }));
                            div()
                                .flex()
                                .justify_between()
                                .items_baseline()
                                .gap(px(12.))
                                .py(px(8.))
                                .when(i + 1 < count, |d| d.border_b_1().border_color(th.border))
                                .child(main)
                                .children(
                                    item.meta
                                        .clone()
                                        .map(|m| div().flex_none().text_color(th.muted).child(m)),
                                )
                        })),
                )
                .into_any_element()
        }
        Panel::Note { title, message } => ui::card(cx)
            .children(title.clone().map(|title| ui::card_title(title, cx)))
            .child(
                div()
                    .max_w(px(620.))
                    .line_height(gpui::relative(1.6))
                    .text_color(ui::theme(cx).muted)
                    .child(message.clone()),
            )
            .into_any_element(),
        Panel::Heatmap { title, years } => heatmap::render(title.as_deref(), years, id, window, cx),
    }
}

/// A row of KPI tiles separated by hairlines.
fn stat_cards(title: Option<&str>, stats: &[Stat], cx: &App) -> AnyElement {
    let th = ui::theme(cx);
    ui::card(cx)
        .children(title.map(|title| ui::card_title(title.to_string(), cx)))
        .child(
            // One row of tiles split by hairlines. Even the narrowest window
            // fits four 130px tiles, so the row never wraps in practice.
            div()
                .flex()
                .flex_wrap()
                .border_1()
                .border_color(th.border)
                .children(stats.iter().enumerate().map(|(i, s)| {
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .flex_basis(px(130.))
                        .min_w(px(130.))
                        .p(px(12.))
                        .bg(th.panel)
                        .when(i > 0, |d| d.border_l_1().border_color(th.border))
                        .child(
                            div()
                                .text_size(px(20.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(th.bright)
                                .child(s.value.clone()),
                        )
                        .child(div().mt(px(3.)).text_color(th.muted).child(s.label.clone()))
                        .children(s.sub.clone().map(|sub| {
                            div()
                                .mt(px(3.))
                                .text_size(px(ui::SMALL))
                                .text_color(th.muted)
                                .opacity(0.8)
                                .child(sub)
                        }))
                })),
        )
        .into_any_element()
}

/// A labelled progress bar against a limit.
fn meter(
    label: &str,
    used: f64,
    limit: Option<f64>,
    sub: Option<&str>,
    caption: Option<&str>,
    cx: &App,
) -> AnyElement {
    let th = ui::theme(cx);
    let pct = limit
        .filter(|l| *l > 0.0)
        .map(|l| (used / l * 100.0).min(100.0))
        .unwrap_or(0.0);
    let caption = caption
        .map(str::to_string)
        .unwrap_or_else(|| format!("{}%", pct.round()));
    ui::card(cx)
        .child(
            div()
                .flex()
                .justify_between()
                .items_start()
                .mb(px(9.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(div().text_color(th.bright).child(label.to_string()))
                        .children(sub.map(|s| {
                            div()
                                .text_size(px(ui::SMALL))
                                .text_color(th.muted)
                                .child(s.to_string())
                        })),
                )
                .child(div().flex_none().text_color(th.text).child(caption)),
        )
        .child(ui::track(pct as f32 / 100.0, 10., cx))
        .into_any_element()
}

/// A panel link: underlined, opens in the default browser.
pub fn link(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    href: String,
    cx: &App,
) -> gpui::Stateful<gpui::Div> {
    let th = ui::theme(cx);
    div()
        .id(id)
        .text_color(th.bright)
        .underline()
        .cursor_pointer()
        .hover(move |s| s.text_color(th.muted))
        .child(text.into())
        .on_click(move |_, _, _| ui::open_url(&href))
}
