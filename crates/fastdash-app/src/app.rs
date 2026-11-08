//! The root view: the sidebar and whichever page fills the content pane.

use std::sync::Arc;

use fastdash_core::engine::config::AppConfig;
use fastdash_core::engine::connector::ConnectorMeta;
use fastdash_core::engine::registry::Registry;
use gpui::{
    div, prelude::*, px, AnyElement, Context, FontWeight, SharedString, Subscription, Window,
};

use crate::i18n::t;
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
    _appearance: Subscription,
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
        let mut root = Root {
            registry,
            config,
            connectors: Vec::new(),
            listed: false,
            active: None,
            page: None,
            _appearance: appearance,
        };
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
                this.connectors = list;
                this.listed = true;
                cx.notify();
            });
        })
        .detach();
    }

    fn select_connector(&mut self, id: String, cx: &mut Context<Self>) {
        self.page = None;
        self.active = Some(id);
        cx.notify();
    }

    fn select_page(&mut self, page: Page, cx: &mut Context<Self>) {
        self.page = Some(page);
        cx.notify();
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
                sidebar_tab(
                    SharedString::from(format!("tab-{id}")),
                    c.meta.name.clone(),
                    th.idle,
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
                        .on_click(
                            cx.listener(|this, _, _, cx| this.select_page(Page::Connectors, cx)),
                        ),
                    )
                    .child(
                        sidebar_tab(
                            "page-settings",
                            t("app.settings"),
                            th.idle,
                            self.page == Some(Page::Settings),
                            &th,
                        )
                        .on_click(
                            cx.listener(|this, _, _, cx| this.select_page(Page::Settings, cx)),
                        ),
                    ),
            )
    }

    fn content(&self, cx: &mut Context<Self>) -> AnyElement {
        match self.page {
            Some(Page::Connectors) => ui::topbar(t("app.connectors"), None, cx).into_any_element(),
            Some(Page::Settings) => ui::topbar(t("app.settings"), None, cx).into_any_element(),
            None => match self.active.as_ref() {
                // Nothing connected yet. Held back until the list has actually
                // answered, so it never flashes on the way to a dashboard.
                None if self.listed => ui::topbar(t("welcome.title"), None, cx).into_any_element(),
                None => div().into_any_element(),
                Some(id) => {
                    let name = self
                        .connectors
                        .iter()
                        .find(|c| &c.meta.id == id)
                        .map(|c| c.meta.name.clone())
                        .unwrap_or_default();
                    div()
                        .flex()
                        .flex_col()
                        .size_full()
                        .child(ui::topbar(name, None, cx))
                        .child(ui::empty(t("app.loading"), cx))
                        .into_any_element()
                }
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
