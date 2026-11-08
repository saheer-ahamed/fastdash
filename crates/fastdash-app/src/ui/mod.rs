//! The design system: every reusable piece of chrome, in one flat terminal
//! look (monospace, monochrome, no radius, no shadows except floating layers).
//!
//! Views compose these and never set a colour of their own, so a theme change
//! restyles everything by construction.

pub mod overlay;
pub mod text_input;
pub mod tooltip;
pub mod tour;

use gpui::{
    div, prelude::*, px, relative, App, Div, ElementId, FontWeight, Hsla, SharedString, Stateful,
};

use crate::theme::{self, Theme};

/// Base text size, set at a 1.5 line height.
pub const TEXT: f32 = 13.;
/// The smaller size used for labels, chips and captions.
pub const SMALL: f32 = 12.;

/// The theme, for views that need a colour directly.
pub fn theme(cx: &App) -> Theme {
    theme::current(cx)
}

/// A content column: the scrolling area under a topbar.
pub fn panels() -> Div {
    div().flex().flex_col().gap(px(12.)).px(px(20.)).py(px(16.))
}

/// The page header: `// title` on the left, optional actions on the right.
/// Always as tall as one carrying a Refresh button, so pages line up.
pub fn topbar(title: impl Into<SharedString>, actions: Option<gpui::AnyElement>, cx: &App) -> Div {
    let t = theme(cx);
    let title: SharedString = title.into();
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_between()
        .px(px(20.))
        .py(px(12.))
        .min_h(px(54.5))
        .border_b_1()
        .border_color(t.border)
        .child(
            div()
                .flex()
                .text_size(px(14.))
                .font_weight(FontWeight::BOLD)
                .child(div().text_color(t.muted).child("// "))
                .child(
                    div()
                        .text_color(t.bright)
                        .child(SharedString::from(title.to_lowercase())),
                ),
        )
        .children(actions.map(|a| div().flex().items_center().gap(px(12.)).child(a)))
}

/// A bordered panel card.
pub fn card(cx: &App) -> Div {
    let t = theme(cx);
    div()
        .flex()
        .flex_col()
        .bg(t.panel)
        .border_1()
        .border_color(t.border)
        .px(px(16.))
        .py(px(14.))
}

/// A card's small uppercase heading.
pub fn card_title(text: impl Into<SharedString>, cx: &App) -> Div {
    let t = theme(cx);
    let text: SharedString = text.into();
    div()
        .mb(px(12.))
        .text_size(px(SMALL))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(t.muted)
        .child(SharedString::from(text.to_uppercase()))
}

/// `## SECTION` with an optional badge, between groups of cards.
pub fn section_heading(
    title: impl Into<SharedString>,
    badge: Option<SharedString>,
    cx: &App,
) -> Div {
    let t = theme(cx);
    let title: SharedString = title.into();
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .mt(px(6.))
        .mb(px(-2.))
        .child(
            div()
                .text_size(px(SMALL))
                .font_weight(FontWeight::BOLD)
                .text_color(t.muted)
                .child(SharedString::from(format!("## {}", title.to_uppercase()))),
        )
        .children(badge.map(|b| {
            div()
                .text_size(px(SMALL))
                .text_color(t.bright)
                .border_1()
                .border_color(t.border_bright)
                .px(px(7.))
                .py(px(1.))
                .child(b)
        }))
}

/// Muted placeholder copy in place of content.
pub fn empty(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .px(px(20.))
        .py(px(32.))
        .text_color(theme(cx).muted)
        .child(text.into())
}

/// The square status dot.
pub fn dot(color: Hsla) -> Div {
    div().flex_none().size(px(7.)).bg(color)
}

/// What a status banner is about.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Warn,
    Err,
}

impl Tone {
    pub fn color(self, t: &Theme) -> Hsla {
        match self {
            Tone::Warn => t.warn,
            Tone::Err => t.err,
        }
    }
}

/// A banner: a message with a coloured left rule, plus optional technical
/// detail underneath (developer mode only - callers decide).
pub fn banner(
    tone: Tone,
    message: impl Into<SharedString>,
    detail: Option<SharedString>,
    cx: &App,
) -> Div {
    let t = theme(cx);
    div()
        .relative()
        .flex()
        .flex_col()
        .pl(px(13.))
        .pr(px(12.))
        .py(px(10.))
        .bg(t.panel)
        .border_1()
        .border_color(t.border_bright)
        .child(left_rule(tone.color(&t)))
        .text_color(t.text)
        .child(message.into())
        .children(detail.map(|d| {
            div()
                .mt(px(6.))
                .text_size(px(SMALL))
                .text_color(t.muted)
                .child(d)
        }))
}

/// A 2px coloured rule over a bordered box's left edge, which GPUI cannot
/// express as a border because a border has one colour for all four sides. The parent must be `relative()`.
pub fn left_rule(color: Hsla) -> Div {
    div()
        .absolute()
        .left(px(-1.))
        .top(px(-1.))
        .bottom(px(-1.))
        .w(px(2.))
        .bg(color)
}

/// The primary button (`.save-btn`).
pub fn button(id: impl Into<ElementId>, label: impl Into<SharedString>, cx: &App) -> Stateful<Div> {
    let t = theme(cx);
    div()
        .id(id)
        .flex_none()
        .px(px(16.))
        .py(px(5.))
        .bg(t.panel_2)
        .border_1()
        .border_color(t.border_bright)
        .text_color(t.bright)
        .cursor_pointer()
        .hover(move |s| s.bg(t.border))
        .child(label.into())
}

/// A primary button that cannot be pressed right now.
pub fn button_disabled(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    cx: &App,
) -> Stateful<Div> {
    let t = theme(cx);
    div()
        .id(id)
        .flex_none()
        .px(px(16.))
        .py(px(5.))
        .bg(t.panel_2)
        .border_1()
        .border_color(t.border_bright)
        .text_color(t.muted)
        .child(label.into())
}

/// An inline text button (`.link-btn`): Replace, Copy, Cancel, Dismiss...
pub fn link_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    cx: &App,
) -> Stateful<Div> {
    let t = theme(cx);
    div()
        .id(id)
        .flex_none()
        .text_color(t.fill)
        .cursor_pointer()
        .hover(|s| s.underline())
        .child(label.into())
}

/// A filter chip (`.chip`), lit when `active`.
pub fn chip(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    active: bool,
    cx: &App,
) -> Stateful<Div> {
    let t = theme(cx);
    let base = div()
        .id(id)
        .flex_none()
        .px(px(12.))
        .py(px(3.))
        .border_1()
        .text_size(px(SMALL))
        .cursor_pointer()
        .child(label.into());
    if active {
        base.bg(t.fill).border_color(t.fill).text_color(t.bg)
    } else {
        base.border_color(t.border_bright)
            .text_color(t.muted)
            .hover(move |s| s.bg(t.panel_2).text_color(t.text))
    }
}

/// One option of a segmented control (`.seg`).
pub fn segment(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    active: bool,
    enabled: bool,
    cx: &App,
) -> Stateful<Div> {
    let t = theme(cx);
    let base = div()
        .id(id)
        .flex_none()
        .px(px(14.))
        .py(px(5.))
        .border_1()
        .child(label.into());
    match (active, enabled) {
        (true, _) => base.bg(t.fill).border_color(t.fill).text_color(t.bg),
        (false, true) => base
            .border_color(t.border_bright)
            .text_color(t.muted)
            .cursor_pointer()
            .hover(move |s| s.bg(t.panel_2).text_color(t.text)),
        (false, false) => base.border_color(t.border_bright).text_color(t.muted),
    }
}

/// A row of segments that wraps.
pub fn segmented() -> Div {
    div().flex().flex_wrap().gap(px(6.))
}

/// A sub-tab under the topbar (`.subtab`), underlined when `active`.
pub fn subtab(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    active: bool,
    cx: &App,
) -> Stateful<Div> {
    let t = theme(cx);
    let base = div()
        .id(id)
        .flex_none()
        .px(px(12.))
        .py(px(6.))
        .mb(px(-1.))
        .border_b_2()
        .cursor_pointer()
        .child(label.into());
    if active {
        base.border_color(t.fill).text_color(t.bright)
    } else {
        base.border_color(gpui::transparent_black())
            .text_color(t.muted)
            .hover(move |s| s.text_color(t.text))
    }
}

/// The strip that holds sub-tabs.
pub fn subtabs(cx: &App) -> Div {
    div()
        .flex()
        .flex_none()
        .flex_wrap()
        .gap(px(2.))
        .px(px(20.))
        .pt(px(8.))
        .border_b_1()
        .border_color(theme(cx).border)
}

/// A labelled form field (`.field`).
pub fn field(label: impl Into<SharedString>, cx: &App) -> Div {
    div().flex().flex_col().w_full().mb(px(12.)).child(
        div()
            .mb(px(5.))
            .text_color(theme(cx).muted)
            .child(label.into()),
    )
}

/// Small muted help copy under a field (`.stored-hint`, `.token-help`).
pub fn hint(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .mt(px(5.))
        .text_size(px(SMALL))
        .text_color(theme(cx).muted)
        .child(text.into())
}

/// A checkbox with its label (`.checkbox`).
pub fn checkbox(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    checked: bool,
    cx: &App,
) -> Stateful<Div> {
    let t = theme(cx);
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(8.))
        .cursor_pointer()
        .text_color(t.text)
        .child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(13.))
                .border_1()
                .border_color(if checked { t.fill } else { t.border_bright })
                .bg(if checked { t.fill } else { t.bg })
                .when(checked, |d| d.child(div().size(px(5.)).bg(t.bg))),
        )
        .child(label.into())
}

/// A horizontal progress track filled to `frac` (0..=1).
pub fn track(frac: f32, height: f32, cx: &App) -> Div {
    let t = theme(cx);
    div()
        .w_full()
        .h(px(height))
        .bg(t.panel_2)
        .border_1()
        .border_color(t.border)
        .child(div().h_full().w(relative(frac.clamp(0.0, 1.0))).bg(t.fill))
}

/// Open a URL in the default browser. Only http(s) ever leaves the app, so a
/// panel link can never launch an arbitrary scheme locally.
pub fn open_url(url: &str) {
    if url.starts_with("http://") || url.starts_with("https://") {
        let _ = open::that(url);
    }
}
