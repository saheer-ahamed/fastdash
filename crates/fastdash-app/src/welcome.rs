//! The first run: what fills the content pane while nothing is connected, so
//! the app opens on a way in rather than on an empty dashboard.
//!
//! It is a router, not a second onboarding: each card hands off to that
//! connector's own setup form. It is driven by the connector registry, and
//! deliberately not gated on a "seen" flag - the condition that shows it is
//! "nothing is connected", which heals itself the moment something is.

use gpui::{div, prelude::*, px, AnyElement, App, ClickEvent, SharedString, Window};

use crate::connectors::TABS;
use crate::i18n::{t, tf};
use crate::ui;

/// The page. `on_connect` receives the connector id whose card was clicked.
pub fn render(
    on_connect: impl Fn(&'static str, &mut Window, &mut App) + Clone + 'static,
    cx: &App,
) -> AnyElement {
    let th = ui::theme(cx);
    let body = if TABS.is_empty() {
        ui::empty(t("app.noConnectors"), cx).into_any_element()
    } else {
        let cards = TABS.iter().map(|tab| {
            let on_connect = on_connect.clone();
            let id = tab.id;
            ui::card(cx)
                .flex_1()
                .flex_basis(px(240.))
                .min_w(px(200.))
                .child(ui::card_title(t(tab.label_key), cx))
                .child(
                    div()
                        .mb(px(14.))
                        .line_height(gpui::relative(1.6))
                        .child(t(tab.blurb_key)),
                )
                // The action sits on the card's bottom edge, so a longer blurb
                // in one card never leaves the row of buttons ragged.
                .child(
                    div().flex().mt_auto().child(
                        ui::button(
                            SharedString::from(format!("connect-{id}")),
                            t("welcome.connect"),
                            cx,
                        )
                        .tooltip(ui::tooltip::text(tf(
                            "welcome.connectAria",
                            &[("name", &t(tab.label_key))],
                        )))
                        .on_click(move |_: &ClickEvent, window, cx| on_connect(id, window, cx)),
                    ),
                )
        });
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(ui::section_heading(t("welcome.connectors"), None, cx))
            .child(div().flex().flex_wrap().gap(px(12.)).children(cards))
            .child(
                div()
                    .mt(px(4.))
                    .max_w(px(620.))
                    .text_size(px(ui::SMALL))
                    .line_height(gpui::relative(1.6))
                    .text_color(th.muted)
                    .child(t("welcome.footnote")),
            )
            .into_any_element()
    };
    div()
        .flex()
        .flex_col()
        .size_full()
        .child(ui::topbar(t("welcome.title"), None, cx))
        .child(
            div()
                .id("welcome")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(
                    ui::panels()
                        .child(
                            div()
                                .max_w(px(620.))
                                .line_height(gpui::relative(1.6))
                                .text_color(th.bright)
                                .child(t("welcome.lede")),
                        )
                        .child(body),
                ),
        )
        .into_any_element()
}
