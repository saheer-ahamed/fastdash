//! A connector's dashboard: topbar, status banner and its panels.
//!
//! Snapshots are kept per (connector, range), so flipping between tabs paints
//! what was already fetched instead of a blank. Every fetch goes through
//! fastdash-core's single fetch path, which also fills the warm cache.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use fastdash_core::engine::cache::SnapshotCache;
use fastdash_core::engine::connector::{ConnectorMeta, Health, Snapshot};
use fastdash_core::engine::range::DateRange;
use fastdash_core::engine::refresh;
use fastdash_core::engine::registry::Registry;
use gpui::{div, prelude::*, px, AnyElement, App, Context, Entity, SharedString, Task, Window};

use crate::i18n::{t, tf};
use crate::range_filter::RangeFilter;
use crate::{panels, prefs, runtime, ui};

/// Cache slot for a snapshot: one per (connector, range).
pub fn snap_key(id: &str, range: &DateRange) -> String {
    format!("{id}|{}_{}", range.start, range.end)
}

/// The cadence a connector asks to be polled at while its tab is watched.
/// Clamped so a misdeclared 0 can never become a busy loop.
fn cadence(meta: &ConnectorMeta) -> Duration {
    Duration::from_secs(meta.default_refresh_secs.max(1))
}

/// Whether a snapshot is still young enough to leave alone.
fn is_fresh(snapshot: Option<&Snapshot>, cadence: Duration) -> bool {
    snapshot.is_some_and(|s| {
        let age = Utc::now().signed_duration_since(s.fetched_at);
        age.to_std().is_ok_and(|age| age < cadence)
    })
}

pub struct Dashboard {
    registry: Arc<Registry>,
    cache: Arc<SnapshotCache>,
    timezone: String,
    range_filter: Entity<RangeFilter>,
    active: Option<ConnectorMeta>,
    range: DateRange,
    /// Whether this dashboard is what the user is looking at: its tab is on
    /// screen and the window has focus. Nothing fetches unless it is.
    watched: bool,
    snapshots: HashMap<String, Snapshot>,
    /// The fetch in flight, if any. Replacing it cancels the old one's update.
    fetch: Option<Task<()>>,
    /// The cadence timer while watched. Dropping it stops the polling dead.
    poll: Option<Task<()>>,
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
            poll: None,
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
        self.fetch = None;
        // The flag belongs to one dashboard's last fetch, so moving to another
        // tab starts clean instead of accusing it of a failure not its own.
        self.failed = false;
        self.reconcile(cx);
    }

    /// Whether the dashboard is on screen in a focused window.
    pub fn set_watched(&mut self, watched: bool, cx: &mut Context<Self>) {
        if watched != self.watched {
            self.watched = watched;
            self.reconcile(cx);
        }
    }

    pub fn set_range(&mut self, range: DateRange, cx: &mut Context<Self>) {
        if range != self.range {
            self.range = range;
            self.fetch = None;
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
            }
            None => self.snapshots.clear(),
        }
        cx.notify();
    }

    /// Keep the dashboard on screen fresh - and only that one. Coming back to a
    /// tab refetches at once only if what we have has aged past the connector's
    /// cadence; otherwise it paints the cache and lets the timer take over.
    /// Only today polls: a past range cannot change, so it is fetched once and
    /// left to the Refresh button.
    fn reconcile(&mut self, cx: &mut Context<Self>) {
        self.poll = None;
        cx.notify();
        let Some(meta) = self.active.clone().filter(|_| self.watched) else {
            return;
        };
        let every = cadence(&meta);
        let key = snap_key(&meta.id, &self.range);
        let is_today = self.range == DateRange::today();
        // Nothing on screen for this view yet: the warm cache may still hold a
        // snapshot from earlier in the session. It only ever holds today.
        if is_today && !self.snapshots.contains_key(&key) {
            if let Some(cached) = self.cache.get(&meta.id) {
                self.snapshots.insert(key.clone(), cached);
            }
        }
        if !is_fresh(self.snapshots.get(&key), every) && self.fetch.is_none() {
            self.fetch_now(cx);
        }
        if is_today {
            self.poll = Some(cx.spawn(async move |this, cx| loop {
                cx.background_executor().timer(every).await;
                let alive = this.update(cx, |this, cx| {
                    // A fetch slower than the cadence never gets a second one
                    // piled on top of it.
                    if this.fetch.is_none() {
                        this.fetch_now(cx);
                    }
                });
                if alive.is_err() {
                    break;
                }
            }));
        }
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
        let job = runtime::spawn(async move {
            refresh::refresh_one(&connector, &cache, timezone, range).await
        });
        self.fetch = Some(cx.spawn(async move |this, cx| {
            let snapshot = job.await;
            let _ = this.update(cx, |this, cx| {
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
                .overflow_y_scroll()
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
