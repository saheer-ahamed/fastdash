//! The root view: the sidebar and whichever page fills the content pane.

use std::sync::Arc;

use chrono::{DateTime, NaiveDate, Utc};
use fastdash_core::engine::config::AppConfig;
use fastdash_core::engine::connector::{ConnectorMeta, Health};
use fastdash_core::engine::range::{ist, today_ist};
use fastdash_core::engine::registry::Registry;
use gpui::{
    div, prelude::*, px, AnyElement, Context, Entity, FontWeight, SharedString, Subscription, Task,
    Window,
};

use crate::connectors::{ConnectorSaved, ConnectorsPage};
use crate::dashboard::Dashboard;
use crate::github_view::GithubView;
use crate::i18n::t;
use crate::range_filter::{RangeChanged, RangeFilter};
use crate::settings::{Settings, SettingsEvent};
use crate::theme::{self, Theme};
use crate::ui;
use crate::{prefs, runtime};

/// One connector as the sidebar sees it: its fixed identity plus whether it is
/// connected right now - the backend's answer, from its own credentials.
#[derive(Clone, Debug)]
pub struct ConnectorEntry {
    pub meta: ConnectorMeta,
    pub configured: bool,
}

/// The two pages pinned to the bottom of the sidebar. `None` in
/// [`Root::page`] means a connector's dashboard is showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Connectors,
    Settings,
}

pub struct Root {
    registry: Arc<Registry>,
    config: AppConfig,
    connectors: Vec<ConnectorEntry>,
    /// Whether the connector list has come back yet. Until it has, "nothing
    /// connected" would be a lie, so nothing that depends on it is shown.
    listed: bool,
    active: Option<String>,
    page: Option<Page>,
    dashboard: Entity<Dashboard>,
    github: Entity<GithubView>,
    /// The Connectors page while it is showing. Mounted fresh on each visit,
    /// so every form starts from saved state.
    connectors_page: Option<(Entity<ConnectorsPage>, Subscription)>,
    /// The Settings page while it is showing, mounted fresh on each visit.
    settings_page: Option<(Entity<Settings>, Subscription)>,
    range_filter: Entity<RangeFilter>,
    /// Whether the window is active. Every fetch in the app is gated on it.
    focused: bool,
    /// The IST day the app believes it is on, re-read as the calendar rolls.
    today: NaiveDate,
    _subscriptions: Vec<Subscription>,
    _midnight: Task<()>,
    /// Which connectors are connected, as of the last list. The taskbar
    /// readout follows it, so connecting or disconnecting one redraws it.
    connected: Option<Vec<String>>,
}

impl Root {
    pub fn new(
        registry: Arc<Registry>,
        config: AppConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // "System" follows the OS: re-resolve the palette when it flips.
        let appearance = cx.observe_window_appearance(window, |_, window, cx| {
            theme::apply(prefs::get(cx).theme, window.appearance(), cx);
        });
        // Fetching is gated on the app being watched: the window is active. A
        // background app makes no network calls at all.
        let activation = cx.observe_window_activation(window, |this, window, cx| {
            this.on_activation(window.is_window_active(), cx);
        });
        let range_filter = cx.new(RangeFilter::new);
        let dashboard = cx.new(|cx| {
            Dashboard::new(
                Arc::clone(&registry),
                config.timezone.clone(),
                range_filter.clone(),
                cx,
            )
        });
        // GitHub renders per-account views that fetch themselves, so the
        // generic dashboard never spends a second round of the same
        // rate-limited API on a snapshot nothing renders.
        let github = cx.new(|cx| GithubView::new(range_filter.clone(), cx));
        // The sidebar dots follow what each dashboard last loaded.
        let dashboard_changed = cx.observe(&dashboard, |_, _, cx| cx.notify());
        let github_changed = cx.observe(&github, |_, _, cx| cx.notify());
        // The range is shared, so switching tabs keeps showing the same days.
        let range_changed = cx.subscribe(&range_filter, |this, _, event: &RangeChanged, cx| {
            let range = event.range.clone();
            this.dashboard
                .update(cx, |d, cx| d.set_range(range.clone(), cx));
            this.github.update(cx, |g, cx| g.set_range(range, cx));
        });
        let focused = window.is_window_active();
        let mut root = Root {
            registry,
            config,
            connectors: Vec::new(),
            listed: false,
            active: None,
            page: None,
            dashboard,
            github,
            connectors_page: None,
            settings_page: None,
            range_filter,
            focused,
            today: today_ist(),
            _subscriptions: vec![
                appearance,
                activation,
                dashboard_changed,
                github_changed,
                range_changed,
            ],
            _midnight: Task::ready(()),
            connected: None,
        };
        root.sync_dashboard(cx);
        root.schedule_midnight(cx);
        root.reload_connectors(cx);
        root
    }

    /// Read which connectors exist and which are connected: once on startup,
    /// and again whenever a connector's settings are saved. `is_configured`
    /// reads the OS keychain, so it runs off the UI thread.
    pub fn reload_connectors(&mut self, cx: &mut Context<Self>) {
        let registry = Arc::clone(&self.registry);
        let config = self.config.clone();
        let job = runtime::spawn_blocking(move || {
            registry
                .all()
                .iter()
                .map(|c| ConnectorEntry {
                    meta: c.meta(),
                    configured: c.is_configured(&config),
                })
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |this, cx| {
            let list = job.await.unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                // Keep the current selection while it is still connected,
                // otherwise fall back to the first one - in the same update as
                // the list, so no frame renders a tab that just disappeared.
                let still_there = this
                    .active
                    .as_ref()
                    .is_some_and(|id| list.iter().any(|c| c.configured && &c.meta.id == id));
                if !still_there {
                    this.active = list
                        .iter()
                        .find(|c| c.configured)
                        .map(|c| c.meta.id.clone());
                }
                let connected: Vec<String> = list
                    .iter()
                    .filter(|c| c.configured)
                    .map(|c| c.meta.id.clone())
                    .collect();
                // Startup fills the readout already; after that it redraws
                // only when what is connected actually changed.
                if this
                    .connected
                    .as_ref()
                    .is_some_and(|prev| *prev != connected)
                {
                    let settings = this.config.taskbar.clone();
                    runtime::detach(async move { fastdash_core::taskbar::apply(&settings).await });
                }
                this.connected = Some(connected);
                this.connectors = list;
                this.listed = true;
                this.sync_dashboard(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn on_activation(&mut self, active: bool, cx: &mut Context<Self>) {
        // A machine that slept across midnight fires the timer late; coming
        // back to the window is the other moment to notice the new day.
        if active {
            self.check_today(cx);
        }
        self.focused = active;
        self.sync_dashboard(cx);
    }

    /// Follow the calendar. The app is long-lived: left running past midnight
    /// it would otherwise keep fetching - and polling - the day it opened on,
    /// with the "Today" chip still lit.
    fn check_today(&mut self, cx: &mut Context<Self>) {
        let now = today_ist();
        if now != self.today {
            self.today = now;
            self.range_filter.update(cx, |f, cx| f.roll_to(now, cx));
        }
    }

    /// Wake at the next IST midnight to roll the date filter over. Re-aimed
    /// after every firing rather than run on a fixed 24h interval, which would
    /// drift off midnight.
    fn schedule_midnight(&mut self, cx: &mut Context<Self>) {
        let wait = until_next_ist_midnight(Utc::now());
        self._midnight = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            let _ = this.update(cx, |this, cx| {
                this.check_today(cx);
                this.schedule_midnight(cx);
            });
        });
    }

    fn select_connector(&mut self, id: String, cx: &mut Context<Self>) {
        self.page = None;
        self.active = Some(id);
        self.sync_dashboard(cx);
        cx.notify();
    }

    fn select_page(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        self.open_page(page, None, window, cx);
    }

    /// Show a pinned page. `connector` lands the Connectors page on one
    /// connector's form - the first-run cards use it.
    pub fn open_page(
        &mut self,
        page: Page,
        connector: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.page = Some(page);
        self.connectors_page = (page == Page::Connectors).then(|| {
            let view = cx.new(|cx| ConnectorsPage::new(connector, window, cx));
            let sub = cx.subscribe(&view, |this, _, event: &ConnectorSaved, cx| {
                this.on_connector_saved(event.0, cx);
            });
            (view, sub)
        });
        self.settings_page = (page == Page::Settings).then(|| {
            let view = cx.new(|cx| Settings::new(window, cx));
            let sub = cx.subscribe(&view, |this, _, event: &SettingsEvent, cx| match event {
                SettingsEvent::LocaleChanged => this.on_locale_changed(cx),
            });
            (view, sub)
        });
        self.sync_dashboard(cx);
        cx.notify();
    }

    /// The language changed. Panel strings are baked into snapshots, so every
    /// cached one is in the old language - dropping them all makes the next
    /// tab opened fetch a fresh copy. Nothing refetches now: Settings is what
    /// is on screen.
    fn on_locale_changed(&mut self, cx: &mut Context<Self>) {
        self.config = fastdash_core::engine::config::load();
        self.dashboard.update(cx, |d, cx| d.forget(None, cx));
        self.github.update(cx, |g, cx| g.clear(cx));
        cx.refresh_windows();
    }

    /// A connector's settings were saved: its cached snapshots describe the old
    /// settings, so drop them - the tab refetches when next opened, not now.
    /// The connector list is re-read too, since connecting or disconnecting is
    /// exactly what makes a sidebar tab appear or vanish.
    fn on_connector_saved(&mut self, id: &str, cx: &mut Context<Self>) {
        self.config = fastdash_core::engine::config::load();
        self.dashboard.update(cx, |d, cx| d.forget(Some(id), cx));
        if id == "github" {
            self.github.update(cx, |g, cx| {
                g.clear(cx);
                g.reload_accounts(cx);
            });
        }
        self.reload_connectors(cx);
    }

    /// The connector whose dashboard is on screen: none while a pinned page
    /// covers it.
    fn live(&self) -> Option<&str> {
        self.active.as_deref().filter(|_| self.page.is_none())
    }

    /// Point the dashboards at what is on screen and tell them whether they
    /// are being watched - the tab is showing and the window has focus.
    fn sync_dashboard(&mut self, cx: &mut Context<Self>) {
        let live = self.live().map(str::to_string);
        let on_github = live.as_deref() == Some("github");
        let meta = live
            .filter(|_| !on_github)
            .and_then(|id| self.connectors.iter().find(|c| c.meta.id == id))
            .map(|c| c.meta.clone());
        let focused = self.focused;
        self.dashboard.update(cx, |d, cx| {
            d.show(meta, cx);
            d.set_watched(focused, cx);
        });
        self.github
            .update(cx, |g, cx| g.set_watched(focused && on_github, cx));
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme::current(cx);
        // Only connected connectors get a tab. The Connectors page still lists
        // every connector on purpose - it is where you go to connect one.
        let tabs = self
            .connectors
            .iter()
            .filter(|c| c.configured)
            .map(|c| {
                let id = c.meta.id.clone();
                let active = self.page.is_none() && self.active.as_deref() == Some(id.as_str());
                let status = if id == "github" {
                    self.github.read(cx).status()
                } else {
                    self.dashboard.read(cx).status(&id)
                };
                let dot = status_color(status, &th);
                sidebar_tab(
                    SharedString::from(format!("tab-{id}")),
                    c.meta.name.clone(),
                    dot,
                    active,
                    &th,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.select_connector(id.clone(), cx)))
            })
            .collect::<Vec<_>>();

        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(208.))
            .h_full()
            .gap(px(4.))
            .px(px(8.))
            .py(px(12.))
            .bg(th.panel)
            .border_r_1()
            .border_color(th.border)
            .child(
                div()
                    .flex()
                    .px(px(8.))
                    .pt(px(6.))
                    .pb(px(12.))
                    .font_weight(FontWeight::BOLD)
                    .child(div().text_color(th.muted).child("$ "))
                    .child(div().text_color(th.bright).child("fastdash")),
            )
            .child(div().flex().flex_col().gap(px(1.)).children(tabs))
            .child(
                div()
                    .mt_auto()
                    .pt(px(8.))
                    .border_t_1()
                    .border_color(th.border)
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .child(
                        sidebar_tab(
                            "page-connectors",
                            t("app.connectors"),
                            th.idle,
                            self.page == Some(Page::Connectors),
                            &th,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.select_page(Page::Connectors, window, cx)
                        })),
                    )
                    .child(
                        sidebar_tab(
                            "page-settings",
                            t("app.settings"),
                            th.idle,
                            self.page == Some(Page::Settings),
                            &th,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.select_page(Page::Settings, window, cx)
                        })),
                    ),
            )
    }

    fn content(&self, cx: &mut Context<Self>) -> AnyElement {
        match self.page {
            Some(Page::Connectors) => match &self.connectors_page {
                Some((page, _)) => page.clone().into_any_element(),
                None => div().into_any_element(),
            },
            Some(Page::Settings) => match &self.settings_page {
                Some((page, _)) => page.clone().into_any_element(),
                None => div().into_any_element(),
            },
            None => match self.active.as_ref() {
                // Nothing connected yet. Held back until the list has actually
                // answered, so it never flashes on the way to a dashboard.
                None if self.listed => {
                    let root = cx.entity().downgrade();
                    crate::welcome::render(
                        move |id, window, cx| {
                            let _ = root.update(cx, |root, cx| {
                                root.open_page(Page::Connectors, Some(id), window, cx)
                            });
                        },
                        cx,
                    )
                }
                None => div().into_any_element(),
                Some(id) if id == "github" => self.github.clone().into_any_element(),
                Some(_) => self.dashboard.clone().into_any_element(),
            },
        }
    }
}

impl Render for Root {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme::current(cx);
        div()
            .flex()
            .size_full()
            .bg(th.bg)
            .text_color(th.text)
            .font_family(crate::fonts::family(cx))
            .text_size(px(ui::TEXT))
            .line_height(gpui::relative(1.5))
            .child(self.sidebar(cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(self.content(cx)),
            )
    }
}

/// A sidebar tab (`.tab`): status dot + name, with a 2px rule when active.
fn sidebar_tab(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    dot: gpui::Hsla,
    active: bool,
    th: &Theme,
) -> gpui::Stateful<gpui::Div> {
    let th = *th;
    let base = div()
        .id(id)
        .relative()
        .flex()
        .items_center()
        .gap(px(9.))
        .px(px(10.))
        .py(px(7.))
        .cursor_pointer()
        .child(ui::dot(dot))
        .child(label.into());
    if active {
        base.bg(th.panel_2).text_color(th.bright).child(
            div()
                .absolute()
                .left_0()
                .top_0()
                .bottom_0()
                .w(px(2.))
                .bg(th.fill),
        )
    } else {
        base.text_color(th.muted)
            .hover(move |s| s.bg(th.panel_2).text_color(th.text))
    }
}

/// The colour of a sidebar dot for what a tab last loaded. A tab not opened
/// yet stays idle rather than claiming a status nothing measured.
pub fn status_color(status: Option<&Health>, th: &Theme) -> gpui::Hsla {
    match status {
        Some(Health::Ok) => th.ok,
        Some(
            Health::NeedsAuth { .. } | Health::RateLimited { .. } | Health::Misconfigured { .. },
        ) => th.warn,
        Some(Health::Error { .. }) => th.err,
        None => th.idle,
    }
}

/// Time from `now` to the next IST midnight, never zero, so a timer built on
/// it always moves forward.
fn until_next_ist_midnight(now: DateTime<Utc>) -> std::time::Duration {
    let local = now.with_timezone(&ist());
    let next = local
        .date_naive()
        .succ_opt()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .and_then(|t| t.and_local_timezone(ist()).single());
    next.and_then(|next| (next - local).to_std().ok())
        .unwrap_or_default()
        .max(std::time::Duration::from_secs(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_midnight_timer_aims_at_the_next_ist_day() {
        // 23:59:30 IST on 2026-11-08 is 18:29:30 UTC.
        let now = DateTime::parse_from_rfc3339("2026-11-08T18:29:30Z").unwrap();
        let wait = until_next_ist_midnight(now.with_timezone(&Utc));
        assert_eq!(wait.as_secs(), 30);
        // Exactly midnight still waits a full day, never zero.
        let now = DateTime::parse_from_rfc3339("2026-11-08T18:30:00Z").unwrap();
        assert_eq!(
            until_next_ist_midnight(now.with_timezone(&Utc)).as_secs(),
            86_400
        );
    }
}
