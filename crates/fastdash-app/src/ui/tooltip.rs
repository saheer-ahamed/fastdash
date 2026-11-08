//! Hover tooltips: a small floating note beside what the pointer rests on.

use gpui::{div, prelude::*, px, AnyView, App, Context, SharedString, Window};

use crate::theme;

pub struct Tooltip {
    text: SharedString,
}

impl Render for Tooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::current(cx);
        div()
            .font_family(crate::fonts::family(cx))
            .max_w(px(360.))
            .px(px(8.))
            .py(px(4.))
            .bg(t.panel)
            .border_1()
            .border_color(t.border_bright)
            .text_size(px(12.))
            .line_height(gpui::relative(1.5))
            .text_color(t.text)
            .shadow(vec![gpui::BoxShadow {
                color: gpui::hsla(0., 0., 0., 0.35),
                offset: gpui::point(px(0.), px(4.)),
                blur_radius: px(12.),
                spread_radius: px(0.),
            }])
            .child(self.text.clone())
    }
}

/// A tooltip builder for `.tooltip(...)` on any stateful element.
pub fn text(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text: SharedString = text.into();
    move |_, cx| cx.new(|_| Tooltip { text: text.clone() }).into()
}
