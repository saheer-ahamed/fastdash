//! App-wide preferences. Connector credentials and options live on the
//! Connectors page; each card here writes only its own config slice.

use std::time::Duration;

use fastdash_core::engine::i18n as core_i18n;
use fastdash_core::taskbar::{self, Metric, TaskbarConfig};
use gpui::{div, prelude::*, px, Context, Entity, EventEmitter, SharedString, Task, Window};

use crate::connectors::patch_config;
use crate::i18n::{t, tf, LOCALES};
use crate::theme::{self, ThemeChoice};
use crate::ui::overlay::Flash;
use crate::ui::text_input::TextInput;
use crate::ui::AxisScroll;
use crate::{prefs, runtime, ui};

/// Taps on the version line that toggle developer mode - the classic "tap the
/// build number" gesture.
const DEV_MODE_TAPS: u32 = 5;
/// An idle gap this long resets the tap count, so stray clicks don't add up.
const TAP_RESET: Duration = Duration::from_secs(2);

/// Every taskbar metric, in the order they appear within a line.
const METRICS: [(Metric, &str); 4] = [
    (Metric::ClaudeSession, "settings.metricClaudeSession"),
    (Metric::ClaudeWeekly, "settings.metricClaudeWeekly"),
    (Metric::GithubOpened, "settings.metricGithubOpened"),
    (Metric::GithubMerged, "settings.metricGithubMerged"),
];

/// Where a metric goes: line 0 (top), line 1 (bottom), or nowhere.
type Slot = Option<usize>;

const SLOTS: [(Slot, &str); 3] = [
    (Some(0), "settings.taskbarTop"),
    (Some(1), "settings.taskbarBottom"),
    (None, "settings.taskbarOff"),
];

fn slot_of(lines: &[Vec<Metric>], metric: Metric) -> Slot {
    lines.iter().take(2).position(|line| line.contains(&metric))
}

/// The two lines with `metric` moved to `slot`. Within a line the order is
/// always [`METRICS`] order, so the readout reads the same however it was set.
fn move_metric(lines: &[Vec<Metric>], metric: Metric, slot: Slot) -> Vec<Vec<Metric>> {
    let slot_for = |m: Metric| if m == metric { slot } else { slot_of(lines, m) };
    (0..2)
        .map(|line| {
            METRICS
                .iter()
                .map(|(m, _)| *m)
                .filter(|m| slot_for(*m) == Some(line))
                .collect()
        })
        .collect()
}

/// Emitted when a setting changes something other views must react to.
pub enum SettingsEvent {
    /// The language changed: every cached snapshot is in the old one.
    LocaleChanged,
}

pub struct Settings {
    timezone: Entity<TextInput>,
    filter_bots: bool,
    locale: String,
    taskbar: TaskbarConfig,
    dev_mode_taps: u32,
    _tap_reset: Option<Task<()>>,
    flash: Entity<Flash>,
}

impl EventEmitter<SettingsEvent> for Settings {}

impl Settings {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let config = fastdash_core::engine::config::load();
        let timezone = config.timezone.clone();
        let placeholder = t("settings.timezonePlaceholder");
        Settings {
            timezone: cx.new(|cx| {
                TextInput::new(cx)
                    .with_text(timezone)
                    .placeholder(placeholder)
            }),
            filter_bots: config.filter_bots,
            locale: config.locale,
            taskbar: config.taskbar,
            dev_mode_taps: 0,
            _tap_reset: None,
            flash: cx.new(|_| Flash::default()),
        }
    }

    fn flash(&self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        let message = message.into();
        self.flash.update(cx, |f, cx| f.show(message, cx));
    }

    fn error(&self, err: String, cx: &mut Context<Self>) {
        self.flash(tf("settings.error", &[("message", &err)]), cx);
    }

    fn set_theme(&mut self, choice: ThemeChoice, window: &mut Window, cx: &mut Context<Self>) {
        prefs::update(cx, |p| p.theme = choice);
        theme::apply(choice, window.appearance(), cx);
        cx.notify();
    }

    fn set_locale(&mut self, locale: &str, cx: &mut Context<Self>) {
        let next = locale.to_string();
        match patch_config(|c| c.locale = next.clone()) {
            Ok(_) => {
                core_i18n::set_locale(&next);
                self.locale = next;
                cx.emit(SettingsEvent::LocaleChanged);
            }
            Err(e) => self.error(e, cx),
        }
        cx.notify();
    }

    fn save_general(&mut self, cx: &mut Context<Self>) {
        let timezone = self.timezone.read(cx).text().trim().to_string();
        let filter_bots = self.filter_bots;
        match patch_config(|c| {
            c.timezone = timezone;
            c.filter_bots = filter_bots;
        }) {
            Ok(_) => self.flash(
                tf("settings.saved", &[("section", &t("settings.general"))]),
                cx,
            ),
            Err(e) => self.error(e, cx),
        }
    }

    /// Save and redraw at once: the readout is on screen the whole time, so the
    /// taskbar itself is the preview. The config write is synchronous, so two
    /// quick clicks can never interleave their read-modify-write and lose one.
    fn apply_taskbar(&mut self, next: TaskbarConfig, cx: &mut Context<Self>) {
        self.taskbar = next.clone();
        cx.notify();
        match patch_config(|c| c.taskbar = next.clone()) {
            Ok(_) => {
                runtime::detach(async move { taskbar::apply(&next).await });
            }
            Err(e) => self.error(e, cx),
        }
    }

    fn tap_version(&mut self, cx: &mut Context<Self>) {
        self._tap_reset = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TAP_RESET).await;
            let _ = this.update(cx, |this, _| this.dev_mode_taps = 0);
        }));
        self.dev_mode_taps += 1;
        let remaining = DEV_MODE_TAPS.saturating_sub(self.dev_mode_taps);
        if remaining == 0 {
            self.dev_mode_taps = 0;
            self._tap_reset = None;
            let next = !prefs::get(cx).dev_mode;
            prefs::update(cx, |p| p.dev_mode = next);
            self.flash(
                if next {
                    t("settings.devModeOn")
                } else {
                    t("settings.devModeOff")
                },
                cx,
            );
            cx.refresh_windows();
        } else if remaining <= DEV_MODE_TAPS - 2 {
            // Only start hinting once they are clearly on the way.
            self.flash(
                tf(
                    "settings.devModeCountdown",
                    &[("n", &remaining.to_string())],
                ),
                cx,
            );
        }
        cx.notify();
    }

    fn general_card(&self, cx: &mut Context<Self>) -> gpui::Div {
        let current_theme = prefs::get(cx).theme;
        let themes = ui::segmented().children(ThemeChoice::ALL.into_iter().map(|choice| {
            ui::segment(
                SharedString::from(format!("theme-{}", choice.label())),
                choice.label(),
                choice == current_theme,
                true,
                cx,
            )
            .on_click(cx.listener(move |this, _, window, cx| this.set_theme(choice, window, cx)))
        }));
        let languages = ui::segmented().children(LOCALES.iter().map(|(id, label)| {
            ui::segment(
                SharedString::from(format!("locale-{id}")),
                *label,
                *id == self.locale,
                true,
                cx,
            )
            .on_click(cx.listener(move |this, _, _, cx| this.set_locale(id, cx)))
        }));
        ui::card(cx)
            .child(ui::card_title(t("settings.general"), cx))
            .child(ui::field(t("settings.theme"), cx).child(themes))
            .child(ui::field(t("settings.language"), cx).child(languages))
            .child(ui::field(t("settings.timezone"), cx).child(self.timezone.clone()))
            .child(
                div().flex().child(
                    ui::checkbox(
                        "filter-bots",
                        t("settings.filterBots"),
                        self.filter_bots,
                        cx,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.filter_bots = !this.filter_bots;
                        cx.notify();
                    })),
                ),
            )
            .child(
                div().flex().mt(px(12.)).child(
                    ui::button("save-general", t("settings.save"), cx)
                        .on_click(cx.listener(|this, _, _, cx| this.save_general(cx))),
                ),
            )
    }

    /// Settings for the Windows taskbar readout: whether it shows, and which
    /// line each number sits on. It is drawn into the Windows taskbar only, so
    /// elsewhere there is nothing to configure.
    fn taskbar_card(&self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        if !cfg!(windows) {
            return None;
        }
        let th = ui::theme(cx);
        let enabled = self.taskbar.enabled;
        let rows = METRICS.iter().enumerate().map(|(i, (metric, label))| {
            let metric = *metric;
            let current = slot_of(&self.taskbar.lines, metric);
            let segments = div()
                .flex()
                .gap(px(6.))
                .children(SLOTS.iter().map(|(slot, key)| {
                    let slot = *slot;
                    let seg = ui::segment(
                        SharedString::from(format!("slot-{i}-{key}")),
                        t(key),
                        current == slot,
                        enabled,
                        cx,
                    );
                    if enabled && current != slot {
                        seg.on_click(cx.listener(move |this, _, _, cx| {
                            let lines = move_metric(&this.taskbar.lines, metric, slot);
                            let next = TaskbarConfig {
                                lines,
                                ..this.taskbar.clone()
                            };
                            this.apply_taskbar(next, cx);
                        }))
                    } else {
                        seg
                    }
                }));
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.))
                .py(px(10.))
                .when(i > 0, |d| d.border_t_1().border_color(th.border))
                .child(t(label))
                .child(segments)
        });
        Some(
            ui::card(cx)
                .child(ui::card_title(t("settings.taskbar"), cx))
                .child(
                    div().flex().child(
                        ui::checkbox("taskbar-enabled", t("settings.taskbarShow"), enabled, cx)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let next = TaskbarConfig {
                                    enabled: !this.taskbar.enabled,
                                    ..this.taskbar.clone()
                                };
                                this.apply_taskbar(next, cx);
                            })),
                    ),
                )
                .child(
                    div()
                        .mt(px(12.))
                        .mb(px(4.))
                        .text_color(th.muted)
                        .child(t("settings.taskbarLede")),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .when(!enabled, |d| d.opacity(0.5))
                        .children(rows),
                ),
        )
    }

    fn about_card(&self, cx: &mut Context<Self>) -> gpui::Div {
        let th = ui::theme(cx);
        let version = format!("v{}", env!("CARGO_PKG_VERSION"));
        ui::card(cx)
            .child(ui::card_title(t("settings.about"), cx))
            .child(
                ui::field(t("settings.version"), cx).mb_0().child(
                    // A tap target that reads as plain text: no button chrome.
                    div().flex().child(
                        div()
                            .id("version-tap")
                            .text_color(th.text)
                            .cursor_pointer()
                            .hover(move |s| s.text_color(th.bright))
                            .child(version)
                            .on_click(cx.listener(|this, _, _, cx| this.tap_version(cx))),
                    ),
                ),
            )
            .children(prefs::get(cx).dev_mode.then(|| {
                div()
                    .mt(px(8.))
                    .text_size(px(ui::SMALL))
                    .text_color(th.muted)
                    .child(t("settings.devModeActive"))
            }))
    }
}

impl Render for Settings {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(ui::topbar(t("app.settings"), None, cx))
            .child(
                div().id("settings").flex_1().min_h_0().scroll_y().child(
                    ui::panels()
                        .child(self.flash.clone())
                        .child(self.general_card(cx))
                        .children(self.taskbar_card(cx))
                        .child(self.about_card(cx)),
                ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moving_a_metric_keeps_canonical_order_within_a_line() {
        let lines = vec![vec![Metric::GithubMerged], vec![Metric::ClaudeSession]];
        let moved = move_metric(&lines, Metric::ClaudeSession, Some(0));
        assert_eq!(moved[0], vec![Metric::ClaudeSession, Metric::GithubMerged]);
        assert!(moved[1].is_empty());
        let off = move_metric(&moved, Metric::GithubMerged, None);
        assert_eq!(off[0], vec![Metric::ClaudeSession]);
        assert_eq!(slot_of(&off, Metric::GithubMerged), None);
    }
}
