//! Floating layers: a full-window dimmed scrim holding a dialog, and the
//! transient success/error banner the forms share.

use std::time::Duration;

use gpui::{
    anchored, deferred, div, hsla, point, prelude::*, px, AnchoredPositionMode, AnyElement,
    Context, FocusHandle, IntoElement, KeyDownEvent, SharedString, Task, Window,
};

use crate::ui;

/// The scrim colour behind a dialog.
fn scrim() -> gpui::Hsla {
    hsla(0., 0., 0., 0.62)
}

/// A dimmed full-window layer above everything, with `content` centred in it.
/// Clicking the scrim or pressing Escape calls `on_dismiss`; clicks inside
/// the content stay inside it. The caller focuses `focus` when it opens the
/// dialog, which is what routes Escape here.
pub fn modal(
    window: &Window,
    focus: &FocusHandle,
    content: impl IntoElement,
    on_dismiss: impl Fn(&mut Window, &mut gpui::App) + 'static,
) -> AnyElement {
    let size = window.viewport_size();
    let on_dismiss = std::rc::Rc::new(on_dismiss);
    let on_key = on_dismiss.clone();
    deferred(
        anchored()
            .position_mode(AnchoredPositionMode::Window)
            .position(point(px(0.), px(0.)))
            .child(
                div()
                    .id("modal-scrim")
                    // Holds keyboard focus while open, so Escape reaches it.
                    .track_focus(focus)
                    .occlude()
                    .w(size.width)
                    .h(size.height)
                    .flex()
                    .items_center()
                    .justify_center()
                    .p(px(24.))
                    .bg(scrim())
                    .on_click(move |_, window, cx| on_dismiss(window, cx))
                    .on_key_down(move |event: &KeyDownEvent, window, cx| {
                        if event.keystroke.key == "escape" {
                            on_key(window, cx);
                        }
                    })
                    .child(
                        div()
                            .id("modal-content")
                            .max_h_full()
                            .flex()
                            // Swallow clicks so only the scrim dismisses.
                            .on_click(|_, _, cx| cx.stop_propagation())
                            .child(content),
                    ),
            ),
    )
    .with_priority(1)
    .into_any_element()
}

/// A transient banner ("GitHub settings saved", "Error: ..."), shown at the
/// top of a page for 2.5 seconds.
#[derive(Default)]
pub struct Flash {
    message: Option<SharedString>,
    timer: Option<Task<()>>,
}

impl Flash {
    pub fn show(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.message = Some(message.into());
        self.timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(2500))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.message = None;
                cx.notify();
            });
        }));
        cx.notify();
    }
}

impl Render for Flash {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = ui::theme(cx);
        match &self.message {
            Some(message) => div()
                .px(px(12.))
                .py(px(10.))
                .bg(th.panel)
                .border_1()
                .border_color(th.border_bright)
                .text_color(th.text)
                .child(message.clone()),
            None => div(),
        }
    }
}
