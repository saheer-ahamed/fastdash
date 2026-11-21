//! A spotlight walkthrough: dims the window, rings one element at a time and
//! explains it in a bubble beside it.
//!
//! Targets register their on-screen bounds through [`anchor`] every frame, so
//! the ring stays welded to its target while the page scrolls or reflows. The
//! dim is four panels around the target rather than one sheet, so the hole is
//! a real hole: the highlighted control stays clickable and typeable while
//! everything around it is inert.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    anchored, canvas, deferred, div, hsla, point, prelude::*, px, size, AnchoredPositionMode,
    AnyElement, App, Bounds, EntityId, FocusHandle, FontWeight, KeyDownEvent, Pixels, ScrollHandle,
    SharedString, Window,
};

use crate::i18n::{t, tf};
use crate::ui;

/// Breathing room between the target and the ring.
const PAD: f32 = 6.;
/// Ring to bubble.
const GAP: f32 = 14.;
/// Bubble width, also what keeps it clear of the window edge.
const WIDTH: f32 = 340.;
/// Minimum distance from the window edge.
const EDGE: f32 = 12.;

/// Where each named target was last painted, refreshed every frame, and the
/// view to re-render when one moves.
#[derive(Clone)]
pub struct Anchors {
    bounds: Rc<RefCell<HashMap<&'static str, Bounds<Pixels>>>>,
    owner: EntityId,
}

impl Anchors {
    pub fn new(owner: EntityId) -> Self {
        Anchors {
            bounds: Rc::default(),
            owner,
        }
    }

    pub fn get(&self, name: &str) -> Option<Bounds<Pixels>> {
        self.bounds.borrow().get(name).copied()
    }
}

/// Wrap `element` so its bounds are recorded under `name`. When they move,
/// the owning view re-renders, which keeps a ring around it in step.
pub fn anchor(anchors: &Anchors, name: &'static str, element: impl IntoElement) -> gpui::Div {
    div().relative().child(element).child(record(anchors, name))
}

/// An invisible layer filling its parent (which must be `relative()` or
/// absolutely positioned) that records the parent's bounds under `name`.
fn record(anchors: &Anchors, name: &'static str) -> impl IntoElement {
    let anchors = anchors.clone();
    canvas(
        move |bounds, _, cx| {
            let moved = anchors.bounds.borrow_mut().insert(name, bounds) != Some(bounds);
            // Notifying mid-paint would be lost with the frame being drawn;
            // deferring lands it once the frame is done.
            if moved {
                let owner = anchors.owner;
                cx.defer(move |cx| cx.notify(owner));
            }
        },
        |_, _, _, _| {},
    )
    // Pinned to the parent's corner: an absolute element with no insets would
    // sit at its in-flow position instead.
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// Where the bubble itself was last painted, so its real height decides
/// whether it fits below its target.
const BUBBLE: &str = "tour-bubble";

pub struct Step {
    pub target: &'static str,
    pub title: SharedString,
    pub body: SharedString,
}

/// What the user did in the bubble.
#[derive(Clone, Copy)]
pub enum TourAction {
    Back,
    Next,
    Close,
}

/// Scroll `scroll`'s container so the target sits in the middle of it, when
/// it is not already fully in view.
pub fn scroll_into_view(scroll: &ScrollHandle, target: Bounds<Pixels>) {
    let view = scroll.bounds();
    if target.top() >= view.top() && target.bottom() <= view.bottom() {
        return;
    }
    let offset = scroll.offset();
    let delta = target.center().y - view.center().y;
    let max = scroll.max_offset().y;
    let y = (offset.y - delta).clamp(-max, px(0.));
    scroll.set_offset(point(offset.x, y));
}

/// The walkthrough layer for step `index` of `steps`.
pub fn render(
    steps: &[Step],
    index: usize,
    anchors: &Anchors,
    focus: &FocusHandle,
    on_action: impl Fn(TourAction, &mut Window, &mut App) + 'static,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let th = ui::theme(cx);
    let Some(step) = steps.get(index) else {
        return div().into_any_element();
    };
    let last = index + 1 >= steps.len();
    let on_action = Rc::new(on_action);
    let viewport = window.viewport_size();
    let (vw, vh) = (viewport.width, viewport.height);

    let target = anchors.get(step.target).map(|b| {
        Bounds::new(
            point(b.left() - px(PAD), b.top() - px(PAD)),
            size(b.size.width + px(PAD * 2.), b.size.height + px(PAD * 2.)),
        )
    });

    // Prefer sitting under the target; flip above only when there is room
    // there and not below, so the bubble never hangs off the window.
    let bubble_h = anchors
        .get(BUBBLE)
        .map(|b| b.size.height)
        .unwrap_or(px(170.));
    let (left, top) = match target {
        None => (
            ((vw - px(WIDTH)) / 2.).max(px(EDGE)),
            ((vh - bubble_h) / 2.).max(px(EDGE)),
        ),
        Some(b) => {
            let fits_below = b.bottom() + px(GAP) + bubble_h + px(EDGE) <= vh;
            let fits_above = b.top() - px(GAP) - bubble_h >= px(EDGE);
            let top = if fits_below || !fits_above {
                (b.bottom() + px(GAP)).min((vh - bubble_h - px(EDGE)).max(px(EDGE)))
            } else {
                b.top() - px(GAP) - bubble_h
            };
            let left = b
                .left()
                .max(px(EDGE))
                .min((vw - px(WIDTH) - px(EDGE)).max(px(EDGE)));
            (left, top)
        }
    };

    let dim = hsla(0., 0., 0., 0.62);
    let panel = |x: Pixels, y: Pixels, w: Pixels, h: Pixels| {
        div()
            .absolute()
            .left(x)
            .top(y)
            .w(w.max(px(0.)))
            .h(h.max(px(0.)))
            .bg(dim)
            .occlude()
    };
    let panels: Vec<gpui::Div> = match target {
        Some(b) => vec![
            panel(px(0.), px(0.), vw, b.top()),
            panel(px(0.), b.bottom(), vw, vh - b.bottom()),
            panel(px(0.), b.top(), b.left(), b.size.height),
            panel(b.right(), b.top(), vw - b.right(), b.size.height),
        ],
        None => vec![panel(px(0.), px(0.), vw, vh)],
    };
    let ring = target.map(|b| {
        div()
            .absolute()
            .left(b.left())
            .top(b.top())
            .w(b.size.width)
            .h(b.size.height)
            .border_1()
            .border_color(th.fill)
    });

    let act = |action: TourAction| {
        let on_action = on_action.clone();
        move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| on_action(action, window, cx)
    };
    let on_key = on_action.clone();
    let bubble = div()
        .id("tour-bubble")
        .absolute()
        .left(left)
        .top(top)
        .w(px(WIDTH).min(vw - px(EDGE * 2.)))
        .pl(px(15.))
        .pr(px(14.))
        .py(px(12.))
        .bg(th.panel)
        .border_1()
        .border_color(th.border_bright)
        .child(ui::left_rule(th.fill))
        .shadow(vec![gpui::BoxShadow {
            color: hsla(0., 0., 0., 0.45),
            offset: point(px(0.), px(6.)),
            blur_radius: px(24.),
            spread_radius: px(0.),
            inset: false,
        }])
        .occlude()
        .child(record(anchors, BUBBLE))
        .child(
            div()
                .text_size(px(11.))
                .text_color(th.muted)
                .child(SharedString::from(
                    tf(
                        "tour.step",
                        &[
                            ("n", &(index + 1).to_string()),
                            ("total", &steps.len().to_string()),
                        ],
                    )
                    .to_uppercase(),
                )),
        )
        .child(
            div()
                .mt(px(6.))
                .mb(px(8.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(th.bright)
                .child(step.title.clone()),
        )
        .child(
            div()
                .line_height(gpui::relative(1.55))
                .text_color(th.text)
                .child(step.body.clone()),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(16.))
                .mt(px(14.))
                .children((!last).then(|| {
                    ui::link_button("tour-skip", t("tour.skip"), cx)
                        .on_click(act(TourAction::Close))
                }))
                .child(div().flex_1())
                .children((index > 0).then(|| {
                    ui::link_button("tour-back", t("tour.back"), cx).on_click(act(TourAction::Back))
                }))
                .child(
                    ui::button(
                        "tour-next",
                        if last { t("tour.done") } else { t("tour.next") },
                        cx,
                    )
                    .on_click(act(if last {
                        TourAction::Close
                    } else {
                        TourAction::Next
                    })),
                ),
        );

    deferred(
        anchored()
            .position_mode(AnchoredPositionMode::Window)
            .position(point(px(0.), px(0.)))
            .child(
                div()
                    .id("tour")
                    .track_focus(focus)
                    .relative()
                    .w(vw)
                    .h(vh)
                    .on_key_down(move |event: &KeyDownEvent, window, cx| {
                        let action = match event.keystroke.key.as_str() {
                            "escape" => Some(TourAction::Close),
                            "right" | "enter" => Some(if last {
                                TourAction::Close
                            } else {
                                TourAction::Next
                            }),
                            "left" => Some(TourAction::Back),
                            _ => None,
                        };
                        if let Some(action) = action {
                            on_key(action, window, cx);
                        }
                    })
                    .children(panels)
                    .children(ring)
                    .child(bubble),
            ),
    )
    .into_any_element()
}
