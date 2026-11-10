//! "How do I make a token?" - the step-by-step guide behind the (i) beside the
//! GitHub token box. A dialog rather than a hover tooltip on purpose: it is
//! seven steps you follow with GitHub open in another window, so it has to
//! stay put while you look away.

use gpui::{div, prelude::*, px, AnyElement, App, FontWeight, SharedString};

use crate::i18n::t;
use crate::ui::{self, AxisScroll, SMALL};

/// The dialog's body. `on_close` dismisses it.
pub fn render(
    docs_url: &'static str,
    on_close: impl Fn(&mut gpui::Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let th = ui::theme(cx);
    let step = |n: usize, title: &str, body: &str, extra: Option<AnyElement>| {
        div()
            .flex()
            .gap(px(8.))
            .mb(px(12.))
            .child(
                div()
                    .flex_none()
                    .w(px(16.))
                    .text_color(th.text)
                    .child(format!("{n}.")),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    // Let long steps wrap instead of widening the dialog.
                    .min_w_0()
                    .child(
                        div()
                            .mb(px(2.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(th.bright)
                            .child(t(title)),
                    )
                    .child(div().text_color(th.text).child(t(body)))
                    .children(extra),
            )
    };
    let scopes = div().flex().flex_col().mt(px(8.)).children(
        [
            ("repo", "patGuide.scopeRepo"),
            ("read:org", "patGuide.scopeOrg"),
            ("read:user", "patGuide.scopeUser"),
        ]
        .into_iter()
        .map(|(name, why)| {
            div()
                .flex()
                .items_start()
                .gap(px(8.))
                .mb(px(6.))
                .text_color(th.muted)
                .child(
                    div()
                        .flex_none()
                        .px(px(5.))
                        .bg(th.panel_2)
                        .border_1()
                        .border_color(th.border)
                        .text_color(th.bright)
                        .child(name),
                )
                .child(div().flex_1().min_w_0().child(t(why)))
        }),
    );
    let note = |title: &str, body: &str| {
        div()
            .mt(px(14.))
            .pt(px(12.))
            .border_t_1()
            .border_color(th.border)
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(th.bright)
                    .child(t(title)),
            )
            .child(div().mt(px(4.)).text_color(th.muted).child(t(body)))
    };

    div()
        .flex()
        .flex_col()
        .w(px(640.))
        .max_w_full()
        .max_h_full()
        .bg(th.panel)
        .border_1()
        .border_color(th.border_bright)
        .shadow(vec![gpui::BoxShadow {
            color: gpui::hsla(0., 0., 0., 0.5),
            offset: gpui::point(px(0.), px(10.)),
            blur_radius: px(40.),
            spread_radius: px(0.),
        }])
        .child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(16.))
                .px(px(16.))
                .py(px(12.))
                .border_b_1()
                .border_color(th.border)
                .child(
                    div()
                        .text_size(px(SMALL))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(th.muted)
                        .child(SharedString::from(t("patGuide.title").to_uppercase())),
                )
                .child(div().flex_1())
                .child(
                    ui::link_button("guide-close", t("patGuide.close"), cx)
                        .on_click(move |_, window, cx| on_close(window, cx)),
                ),
        )
        .child(
            div()
                .id("guide-body")
                .flex_1()
                .min_h_0()
                .scroll_y()
                .px(px(16.))
                .pt(px(14.))
                .pb(px(16.))
                .line_height(gpui::relative(1.6))
                .child(
                    div()
                        .mb(px(14.))
                        .text_color(th.text)
                        .child(t("patGuide.lede")),
                )
                .child(step(1, "patGuide.s1Title", "patGuide.s1Body", None))
                .child(step(2, "patGuide.s2Title", "patGuide.s2Body", None))
                .child(step(3, "patGuide.s3Title", "patGuide.s3Body", None))
                .child(step(4, "patGuide.s4Title", "patGuide.s4Body", None))
                .child(step(
                    5,
                    "patGuide.s5Title",
                    "patGuide.s5Body",
                    Some(scopes.into_any_element()),
                ))
                .child(step(6, "patGuide.s6Title", "patGuide.s6Body", None))
                .child(step(7, "patGuide.s7Title", "patGuide.s7Body", None))
                .child(note(
                    "patGuide.fineGrainedTitle",
                    "patGuide.fineGrainedBody",
                ))
                .child(note("patGuide.safetyTitle", "patGuide.safetyBody"))
                .child(
                    div().mt(px(12.)).child(
                        ui::link_button("guide-docs", t("patGuide.docs"), cx)
                            .on_click(move |_, _, _| ui::open_url(docs_url)),
                    ),
                ),
        )
        .into_any_element()
}
