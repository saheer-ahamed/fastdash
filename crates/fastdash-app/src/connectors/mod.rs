//! The Connectors page: one sub-tab per connector, each owning its own form
//! and its own Save button. Saving one connector never touches another's
//! settings - every save re-reads the config and writes back only its slice.
//!
//! Add a connector: write its form (a view emitting [`FormEvent`]) and list it
//! in [`TABS`]. The page renders a sub-tab per entry and the first-run page a
//! card per entry, so neither needs editing.

mod claude;
mod github;
mod sentry;
mod token_guide;

use fastdash_core::engine::config::{self, AppConfig};
use gpui::{
    div, prelude::*, px, AnyView, App, Context, Entity, EventEmitter, SharedString, Subscription,
    Window,
};

use crate::i18n::{t, tf};
use crate::ui::overlay::Flash;
use crate::ui::text_input::TextInput;
use crate::ui::{self, SMALL};

/// One connector's entry on the Connectors page and the first-run page.
pub struct ConnectorTab {
    /// Connector id, matching fastdash-core's registry.
    pub id: &'static str,
    /// i18n key for the sub-tab label.
    pub label_key: &'static str,
    /// i18n key for the one line saying what this connector puts on the
    /// dashboard, shown on the first-run page.
    pub blurb_key: &'static str,
}

pub const TABS: &[ConnectorTab] = &[
    ConnectorTab {
        id: "claude",
        label_key: "settings.claude",
        blurb_key: "welcome.claudeBlurb",
    },
    ConnectorTab {
        id: "github",
        label_key: "settings.github",
        blurb_key: "welcome.githubBlurb",
    },
    ConnectorTab {
        id: "sentry",
        label_key: "settings.sentry",
        blurb_key: "welcome.sentryBlurb",
    },
];

/// What a connector form tells the page.
pub enum FormEvent {
    /// A transient success line.
    Flash(SharedString),
    /// A transient error line.
    Error(String),
    /// The connector's settings or credentials changed: its cached dashboards
    /// are stale and whether it is connected may have flipped.
    Saved,
}

/// Emitted by the page when a connector's settings were saved.
pub struct ConnectorSaved(pub &'static str);

/// Re-read the persisted config, apply `f` to the caller's slice, and write
/// it back - so two forms saved in any order both survive.
pub fn patch_config(f: impl FnOnce(&mut AppConfig)) -> Result<AppConfig, String> {
    let mut config = config::load();
    f(&mut config);
    config::save(&config).map_err(|e| e.to_string())?;
    Ok(config)
}

pub struct ConnectorsPage {
    active: usize,
    form: AnyView,
    flash: Entity<Flash>,
    _form_events: Subscription,
}

impl EventEmitter<ConnectorSaved> for ConnectorsPage {}

impl ConnectorsPage {
    /// `initial` is the connector to land on, for a caller that already knows
    /// which one the user asked for (a first-run card). An id that matches no
    /// tab falls back to the first rather than blanking the page.
    pub fn new(initial: Option<&str>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let active = initial
            .and_then(|id| TABS.iter().position(|t| t.id == id))
            .unwrap_or(0);
        let flash = cx.new(|_| Flash::default());
        let (form, sub) = Self::mount(active, window, cx);
        ConnectorsPage {
            active,
            form,
            flash,
            _form_events: sub,
        }
    }

    /// A fresh form for tab `ix`, so each one starts from saved state.
    fn mount(ix: usize, window: &mut Window, cx: &mut Context<Self>) -> (AnyView, Subscription) {
        let id = TABS[ix].id;
        let handler = move |this: &mut Self, event: &FormEvent, cx: &mut Context<Self>| match event
        {
            FormEvent::Flash(msg) => {
                let msg = msg.clone();
                this.flash.update(cx, |f, cx| f.show(msg, cx));
            }
            FormEvent::Error(err) => {
                let msg = tf("settings.error", &[("message", err)]);
                this.flash.update(cx, |f, cx| f.show(msg, cx));
            }
            FormEvent::Saved => cx.emit(ConnectorSaved(id)),
        };
        match id {
            "claude" => {
                let form = cx.new(|cx| claude::ClaudeForm::new(window, cx));
                let sub = cx.subscribe(&form, move |this, _, e, cx| handler(this, e, cx));
                (form.into(), sub)
            }
            "github" => {
                let form = cx.new(|cx| github::GithubForm::new(window, cx));
                let sub = cx.subscribe(&form, move |this, _, e, cx| handler(this, e, cx));
                (form.into(), sub)
            }
            _ => {
                let form = cx.new(|cx| sentry::SentryForm::new(window, cx));
                let sub = cx.subscribe(&form, move |this, _, e, cx| handler(this, e, cx));
                (form.into(), sub)
            }
        }
    }

    fn select(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix == self.active {
            return;
        }
        self.active = ix;
        let (form, sub) = Self::mount(ix, window, cx);
        self.form = form;
        self._form_events = sub;
        cx.notify();
    }
}

impl Render for ConnectorsPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let subtabs = ui::subtabs(cx).children(TABS.iter().enumerate().map(|(ix, tab)| {
            ui::subtab(tab.id, t(tab.label_key), ix == self.active, cx)
                .on_click(cx.listener(move |this, _, window, cx| this.select(ix, window, cx)))
        }));
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(ui::topbar(t("app.connectors"), None, cx))
            .child(subtabs)
            .child(
                div()
                    .id("connector-form")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        ui::panels()
                            .child(self.flash.clone())
                            .child(self.form.clone()),
                    ),
            )
    }
}

/// One account block inside a multi-account editor (`.account-card`).
pub fn account_card(cx: &App) -> gpui::Div {
    let th = ui::theme(cx);
    div()
        .flex()
        .flex_col()
        .border_1()
        .border_color(th.border)
        .bg(th.bg)
        .px(px(14.))
        .py(px(12.))
        .mb(px(12.))
}

/// An account block's header: an uppercase caption with an action on the right.
pub fn account_head(
    caption: SharedString,
    action: Option<gpui::AnyElement>,
    cx: &App,
) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .mb(px(8.))
        .text_size(px(SMALL))
        .child(
            div()
                .text_color(ui::theme(cx).muted)
                .child(SharedString::from(caption.to_uppercase())),
        )
        .children(action)
}

/// Muted help paragraphs under a field (`.token-help`).
pub fn token_help(lines: Vec<SharedString>, cx: &App) -> gpui::Div {
    let th = ui::theme(cx);
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .mt(px(6.))
        .text_size(px(SMALL))
        .text_color(th.muted)
        .children(lines.into_iter().map(|l| div().child(l)))
}

/// "A secret is already stored": a masked, read-only field and Replace.
pub fn stored_token(field: &Entity<TextInput>, replace: gpui::Stateful<gpui::Div>) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .child(div().flex_1().child(field.clone()))
        .child(replace)
}

/// A masked read-only field showing that a secret exists without ever
/// holding it - the real secret is never read back into the UI.
pub fn stored_field(cx: &mut App) -> Entity<TextInput> {
    cx.new(|cx| {
        let mut input = TextInput::new(cx).masked().with_text("storedtoken");
        input.set_read_only(true, cx);
        input
    })
}

/// A plain text field.
pub fn text_field(
    text: impl Into<SharedString>,
    placeholder: impl Into<SharedString>,
    cx: &mut App,
) -> Entity<TextInput> {
    let text = text.into();
    let placeholder = placeholder.into();
    cx.new(|cx| TextInput::new(cx).with_text(text).placeholder(placeholder))
}

/// A masked field for a new secret.
pub fn secret_field(placeholder: impl Into<SharedString>, cx: &mut App) -> Entity<TextInput> {
    let placeholder = placeholder.into();
    cx.new(|cx| TextInput::new(cx).masked().placeholder(placeholder))
}

/// Split an organizations field: commas or whitespace separate entries.
pub fn parse_list(raw: &str) -> Vec<String> {
    raw.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_split_on_commas_and_whitespace() {
        assert_eq!(parse_list(" a, b  c,,d "), vec!["a", "b", "c", "d"]);
        assert!(parse_list("  ").is_empty());
    }

    #[test]
    fn every_tab_has_strings() {
        for tab in TABS {
            assert_ne!(t(tab.label_key).as_ref(), tab.label_key);
            assert_ne!(t(tab.blurb_key).as_ref(), tab.blurb_key);
        }
    }
}
