//! The date filter shown above every dashboard: preset chips plus a custom
//! start/end pair. Presets resolve to concrete dates here, so a connector only
//! ever receives a span.
//!
//! "Today" is fastdash-core's today (IST), the same day every connector counts
//! from. Taking the machine's local date instead would, near midnight in any
//! timezone behind IST, ask for a day the connectors call "yesterday".

use chrono::{Duration, NaiveDate};
use fastdash_core::engine::range::{today_ist, DateRange};
use gpui::{div, prelude::*, px, Context, Entity, EventEmitter, Subscription, Window};

use crate::i18n::t;
use crate::ui::text_input::{InputEvent, TextInput};
use crate::ui::{self, SMALL};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    Today,
    Yesterday,
    Last7,
    Last30,
    Custom,
}

impl Preset {
    /// Chip order. "Custom" is last and reveals the two date fields.
    pub const ALL: [Preset; 5] = [
        Preset::Today,
        Preset::Yesterday,
        Preset::Last7,
        Preset::Last30,
        Preset::Custom,
    ];

    fn label_key(self) -> &'static str {
        match self {
            Preset::Today => "range.today",
            Preset::Yesterday => "range.yesterday",
            Preset::Last7 => "range.last7",
            Preset::Last30 => "range.last30",
            Preset::Custom => "range.custom",
        }
    }

    /// The span this preset means on `today`. "Custom" has no fixed span, so it
    /// keeps `current`.
    pub fn resolve(self, today: NaiveDate, current: &DateRange) -> DateRange {
        let span = |back: i64| DateRange {
            start: today - Duration::days(back),
            end: today,
        };
        match self {
            Preset::Today => span(0),
            Preset::Yesterday => {
                let y = today - Duration::days(1);
                DateRange { start: y, end: y }
            }
            Preset::Last7 => span(6),
            Preset::Last30 => span(29),
            Preset::Custom => current.clone(),
        }
    }
}

/// A custom range typed into the two fields, if it is a real span that does
/// not reach past today.
fn parse_custom(start: &str, end: &str, today: NaiveDate) -> Option<DateRange> {
    let parse = |s: &str| NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok();
    let (start, end) = (parse(start)?, parse(end)?);
    (start <= end && end <= today).then_some(DateRange { start, end })
}

/// Emitted when the user picks a new range.
pub struct RangeChanged {
    pub range: DateRange,
    pub preset: Preset,
}

pub struct RangeFilter {
    range: DateRange,
    preset: Preset,
    start: Entity<TextInput>,
    end: Entity<TextInput>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<RangeChanged> for RangeFilter {}

impl RangeFilter {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let range = DateRange::today();
        let start = cx.new(|cx| TextInput::new(cx).small().placeholder("YYYY-MM-DD"));
        let end = cx.new(|cx| TextInput::new(cx).small().placeholder("YYYY-MM-DD"));
        // Re-render as the user types, so Apply enables the moment the span is
        // valid; Enter in either field applies it.
        let subscriptions = [&start, &end]
            .into_iter()
            .map(|input| {
                cx.subscribe(input, |this, _, event, cx| match event {
                    InputEvent::Changed => cx.notify(),
                    InputEvent::Submit => this.apply_custom(cx),
                })
            })
            .collect();
        RangeFilter {
            range,
            preset: Preset::Today,
            start,
            end,
            _subscriptions: subscriptions,
        }
    }

    pub fn range(&self) -> &DateRange {
        &self.range
    }

    /// Follow the calendar: every preset but "custom" is relative to today, so
    /// a day rolling over re-resolves it. Returns whether the span moved.
    pub fn roll_to(&mut self, today: NaiveDate, cx: &mut Context<Self>) -> bool {
        if self.preset == Preset::Custom {
            return false;
        }
        let next = self.preset.resolve(today, &self.range);
        if next == self.range {
            return false;
        }
        self.range = next.clone();
        cx.emit(RangeChanged {
            range: next,
            preset: self.preset,
        });
        cx.notify();
        true
    }

    fn pick(&mut self, preset: Preset, cx: &mut Context<Self>) {
        let range = preset.resolve(today_ist(), &self.range);
        if preset == Preset::Custom {
            // Seed the fields from whatever is selected as the panel opens.
            let (start, end) = (range.start.to_string(), range.end.to_string());
            self.start.update(cx, |i, cx| i.set_text(start, cx));
            self.end.update(cx, |i, cx| i.set_text(end, cx));
        }
        self.set(range, preset, cx);
    }

    fn apply_custom(&mut self, cx: &mut Context<Self>) {
        let (start, end) = (
            self.start.read(cx).text().to_string(),
            self.end.read(cx).text().to_string(),
        );
        if let Some(range) = parse_custom(&start, &end, today_ist()) {
            self.set(range, Preset::Custom, cx);
        }
    }

    fn set(&mut self, range: DateRange, preset: Preset, cx: &mut Context<Self>) {
        self.preset = preset;
        self.range = range.clone();
        cx.emit(RangeChanged { range, preset });
        cx.notify();
    }
}

impl Render for RangeFilter {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = ui::theme(cx);
        let chips = div()
            .flex()
            .flex_wrap()
            .gap(px(6.))
            .children(Preset::ALL.into_iter().map(|p| {
                ui::chip(p.label_key(), t(p.label_key()), p == self.preset, cx)
                    .on_click(cx.listener(move |this, _, _, cx| this.pick(p, cx)))
            }));

        let custom = (self.preset == Preset::Custom).then(|| {
            let valid = parse_custom(
                self.start.read(cx).text(),
                self.end.read(cx).text(),
                today_ist(),
            )
            .is_some();
            let field = |label: &str, input: &Entity<TextInput>| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(px(SMALL))
                    .child(div().text_color(th.muted).child(t(label)))
                    .child(div().w(px(104.)).child(input.clone()))
            };
            let apply = div()
                .id("range-apply")
                .px(px(12.))
                .py(px(3.))
                .bg(th.panel_2)
                .border_1()
                .border_color(th.border_bright)
                .text_size(px(SMALL))
                .child(t("range.apply"));
            let apply = if valid {
                apply
                    .text_color(th.bright)
                    .cursor_pointer()
                    .hover(move |s| s.bg(th.border))
                    .on_click(cx.listener(|this, _, _, cx| this.apply_custom(cx)))
            } else {
                apply.text_color(th.muted)
            };
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_x(px(10.))
                .gap_y(px(6.))
                .child(field("range.from", &self.start))
                .child(field("range.to", &self.end))
                .child(apply)
        });

        div()
            .flex()
            .flex_none()
            .flex_wrap()
            .items_start()
            .gap_x(px(10.))
            .gap_y(px(6.))
            .px(px(20.))
            .pt(px(10.))
            .child(
                // Lines up with the chip row's text, not with the middle of the
                // two-row body once the custom dates are open.
                div()
                    .pt(px(4.))
                    .text_size(px(SMALL))
                    .text_color(th.muted)
                    .child(t("range.label").to_lowercase()),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(chips)
                    .children(custom),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn presets_resolve_against_the_given_today() {
        let today = day("2026-11-09");
        let cur = DateRange::today();
        assert_eq!(Preset::Today.resolve(today, &cur).start, today);
        let y = Preset::Yesterday.resolve(today, &cur);
        assert_eq!((y.start, y.end), (day("2026-11-08"), day("2026-11-08")));
        assert_eq!(Preset::Last7.resolve(today, &cur).start, day("2026-11-03"));
        assert_eq!(Preset::Last30.resolve(today, &cur).start, day("2026-10-11"));
        assert_eq!(Preset::Custom.resolve(today, &cur), cur);
    }

    #[test]
    fn a_custom_span_must_be_ordered_and_not_reach_the_future() {
        let today = day("2026-11-09");
        assert!(parse_custom("2026-11-01", "2026-11-09", today).is_some());
        assert!(parse_custom(" 2026-11-01 ", "2026-11-01", today).is_some());
        assert!(parse_custom("2026-11-05", "2026-11-01", today).is_none());
        assert!(parse_custom("2026-11-01", "2026-11-10", today).is_none());
        assert!(parse_custom("2026-11", "2026-11-09", today).is_none());
    }
}
