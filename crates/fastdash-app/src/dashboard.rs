//! A connector's dashboard: topbar, status banner and its panels.
//!
//! Snapshots are kept per (connector, range), so flipping between tabs paints
//! what was already fetched instead of a blank. Every fetch goes through
//! fastdash-core's single fetch path, which also fills the warm cache.

use std::collections::HashMap;
use std::sync::Arc;

use fastdash_core::engine::cache::SnapshotCache;
use fastdash_core::engine::connector::{ConnectorMeta, Health, Snapshot};
use fastdash_core::engine::range::DateRange;
use fastdash_core::engine::refresh::{self, should_fetch};
use fastdash_core::engine::registry::Registry;
use gpui::{div, prelude::*, px, AnyElement, App, Context, Entity, SharedString, Task, Window};

use crate::i18n::{t, tf};
use crate::range_filter::RangeFilter;
use crate::ui::AxisScroll;
use crate::{panels, prefs, runtime, ui};

/// Cache slot for a snapshot: one per (connector, range).
pub fn snap_key(id: &str, range: &DateRange) -> String {
    format!("{id}|{}_{}", range.start, range.end)
}

pub struct Dashboard {
    registry: Arc<Registry>,
    cache: Arc<SnapshotCache>,
    timezone: String,
    range_filter: Entity<RangeFilter>,
    active: Option<ConnectorMeta>,
    range: DateRange,
    /// Whether this dashboard's tab is the one on screen. Focus does not matter:
    /// a backgrounded window keeps a fetch that was already asked for, and
    /// gaining focus does not start one.
    watched: bool,
    snapshots: HashMap<String, Snapshot>,
    /// The fetch in flight, if any. Replacing it aborts the previous request.
    fetch: Option<Task<()>>,
    /// Bumped every time a fetch starts or is cancelled, so a late result from
    /// a request the user already left cannot clear the one that replaced it.
    fetch_gen: u64,
    /// Whether the last fetch of this dashboard failed outright. The cached
    /// snapshot stays on screen; the topbar says it is not what it looks like.
    failed: bool,
}

impl Dashboard {
    pub fn new(
        registry: Arc<Registry>,
        timezone: String,
        range_filter: Entity<RangeFilter>,
        cx: &mut Context<Self>,
    ) -> Self {
        let range = range_filter.read(cx).range().clone();
        Dashboard {
            registry,
            cache: Arc::new(SnapshotCache::new()),
            timezone,
            range_filter,
            active: None,
            range,
            watched: false,
            snapshots: HashMap::new(),
            fetch: None,
            fetch_gen: 0,
            failed: false,
        }
    }

    /// Show `meta`'s dashboard (or nothing).
    pub fn show(&mut self, meta: Option<ConnectorMeta>, cx: &mut Context<Self>) {
        let changed = self.active.as_ref().map(|m| &m.id) != meta.as_ref().map(|m| &m.id);
        if !changed {
            return;
        }
        self.active = meta;
        self.cancel_fetch();
        // The flag belongs to one dashboard's last fetch, so moving to another
        // tab starts clean instead of accusing it of a failure not its own.
        self.failed = false;
        self.reconcile(cx);
    }

    /// Whether the dashboard's tab is the one on screen.
    pub fn set_watched(&mut self, watched: bool, cx: &mut Context<Self>) {
        if watched != self.watched {
            self.watched = watched;
            self.reconcile(cx);
        }
    }

    pub fn set_range(&mut self, range: DateRange, cx: &mut Context<Self>) {
        if range != self.range {
            self.range = range;
            self.cancel_fetch();
            self.failed = false;
            self.reconcile(cx);
        }
    }

    /// Forget every cached snapshot of `id` (its settings changed), or of every
    /// connector when `id` is `None` (the language changed). The next look at
    /// one refetches rather than painting stale panels.
    pub fn forget(&mut self, id: Option<&str>, cx: &mut Context<Self>) {
        match id {
            Some(id) => {
                let prefix = format!("{id}|");
                self.snapshots.retain(|k, _| !k.starts_with(&prefix));
                self.cache.remove(id);
            }
            None => {
                self.snapshots.clear();
                self.cache.clear();
            }
        }
        cx.notify();
    }

    /// Fetch the dashboard on screen when it has no snapshot yet. A tab the
    /// user already opened keeps what it has until they hit Refresh.
    fn reconcile(&mut self, cx: &mut Context<Self>) {
        cx.notify();
        let Some(meta) = self.active.clone().filter(|_| self.watched) else {
            self.cancel_fetch();
            return;
        };
        let key = snap_key(&meta.id, &self.range);
        // Nothing on screen for this view yet: the warm cache may still hold a
        // snapshot from earlier in the session. It only ever holds today.
        if self.range == DateRange::today() && !self.snapshots.contains_key(&key) {
            if let Some(cached) = self.cache.get(&meta.id) {
                self.snapshots.insert(key.clone(), cached);
            }
        }
        if should_fetch(self.snapshots.contains_key(&key), false) && self.fetch.is_none() {
            self.fetch_now(cx);
        }
    }

    fn cancel_fetch(&mut self) {
        self.fetch_gen = self.fetch_gen.wrapping_add(1);
        self.fetch = None;
    }

    /// The health of what `id`'s tab last loaded, for its sidebar dot.
    pub fn status(&self, id: &str) -> Option<&Health> {
        self.snapshots
            .get(&snap_key(id, &self.range))
            .map(|s| &s.status)
    }

    fn current(&self) -> Option<&Snapshot> {
        let meta = self.active.as_ref()?;
        self.snapshots.get(&snap_key(&meta.id, &self.range))
    }

    pub fn fetch_now(&mut self, cx: &mut Context<Self>) {
        let Some(meta) = self.active.clone() else {
            return;
        };
        let Some(connector) = self.registry.get(&meta.id) else {
            return;
        };
        let cache = Arc::clone(&self.cache);
        let range = self.range.clone();
        let key = snap_key(&meta.id, &range);
        let timezone = self.timezone.clone();
        self.fetch_gen = self.fetch_gen.wrapping_add(1);
        let gen = self.fetch_gen;
        // Abortable: dropping this task (a newer refresh, or leaving the tab)
        // cancels the request instead of letting it finish in the background.
        let job = runtime::spawn_abortable(async move {
            refresh::refresh_one(&connector, &cache, timezone, range).await
        });
        self.fetch = Some(cx.spawn(async move |this, cx| {
            let snapshot = job.await;
            let _ = this.update(cx, |this, cx| {
                if this.fetch_gen != gen {
                    return;
                }
                match snapshot {
                    // Filed under the range it covers, so a fetch that lands
                    // after the filter moved cannot overwrite the view.
                    Some(snapshot) => {
                        this.snapshots.insert(key, snapshot);
                        this.failed = false;
                    }
                    None => this.failed = true,
                }
                this.fetch = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn topbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let th = ui::theme(cx);
        let name = self
            .active
            .as_ref()
            .map(|m| m.name.clone())
            .unwrap_or_default();
        let loading = self.fetch.is_some();
        let status = if self.failed && !loading {
            Some(div().text_color(th.warn).child(t("app.refreshFailed")))
        } else {
            self.current().map(|s| {
                div()
                    .text_color(th.muted)
                    .child(tf("app.updated", &[("time", &fetched_label(s))]))
            })
        };
        let button = refresh_button("refresh", loading, cx).when(!loading, |b| {
            b.on_click(cx.listener(|this, _, _, cx| this.fetch_now(cx)))
        });
        ui::topbar(
            name,
            Some(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .children(status)
                    .child(button)
                    .into_any_element(),
            ),
            cx,
        )
    }
}

impl Render for Dashboard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _span = fastdash_core::engine::perf::span("ui.dashboard");
        let dev_mode = prefs::get(cx).dev_mode;
        let body: AnyElement = match (self.current(), &self.active) {
            (Some(snapshot), Some(meta)) => {
                let scope = snap_key(&meta.id, &self.range);
                let banner = panels::status_banner(&snapshot.status, dev_mode, cx);
                let panels = panels::render_panels(snapshot, &scope, window, cx);
                snapshot_view(banner, panels, scope.into())
            }
            _ => ui::empty(t("app.loading"), cx).into_any_element(),
        };
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(self.topbar(cx))
            .child(self.range_filter.clone())
            .child(body)
    }
}

/// The status banner pinned above a scrolling column of panels.
pub fn snapshot_view(
    banner: Option<AnyElement>,
    panels: Vec<AnyElement>,
    scope: SharedString,
) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .children(banner.map(|b| div().flex_none().px(px(20.)).pt(px(16.)).child(b)))
        .child(
            div()
                .id(scope)
                .flex_1()
                .min_h_0()
                .scroll_y()
                .child(ui::panels().children(panels)),
        )
        .into_any_element()
}

/// The topbar's Refresh button (`.refresh`). While a fetch runs it is dimmed
/// and says so.
pub fn refresh_button(id: &'static str, loading: bool, cx: &App) -> gpui::Stateful<gpui::Div> {
    let th = ui::theme(cx);
    let base = div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(6.))
        .px(px(12.))
        .py(px(4.))
        .bg(th.panel_2)
        .border_1()
        .border_color(th.border_bright)
        .text_color(th.text);
    if loading {
        base.opacity(0.4).child(t("app.refreshing"))
    } else {
        base.cursor_pointer()
            .hover(move |s| s.bg(th.border).text_color(th.bright))
            .child(t("app.refresh"))
    }
}

/// When a snapshot was fetched, as a local wall-clock time.
pub fn fetched_label(snapshot: &Snapshot) -> String {
    snapshot
        .fetched_at
        .with_timezone(&chrono::Local)
        .format("%I:%M:%S %p")
        .to_string()
}
