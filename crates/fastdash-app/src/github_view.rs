//! The GitHub dashboard: one sub-tab per connected account, with an org filter
//! (All + each org) inside the account.
//!
//! Each (account, org, range) view fetches through the connector's own
//! `fetch_account`, which cancels whatever GitHub fetch was still in flight -
//! flipping through sub-tabs costs one fetch, not one per click. The state
//! lives for the life of the app, so leaving and re-entering the tab paints
//! the cache at once and refreshes quietly behind it.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use chrono::Utc;
use fastdash_core::connectors::github::{fetch_account, SUPERSEDED};
use fastdash_core::engine::config::{self, GithubAccount};
use fastdash_core::engine::connector::{Health, Snapshot};
use fastdash_core::engine::range::DateRange;
use gpui::{div, prelude::*, px, AnyElement, Context, Entity, SharedString, Task, Window};

use crate::dashboard::{fetched_label, snapshot_view};
use crate::i18n::{t, tf};
use crate::range_filter::RangeFilter;
use crate::{assets, panels, prefs, runtime, ui};

/// How long a view's snapshot counts as fresh, and the polling cadence while
/// the GitHub tab is watched.
const REFRESH: Duration = Duration::from_secs(60);

/// Stable cache key for an (account, org, range) view. A space cannot appear
/// in a label or a stored org, so it is a safe separator.
fn view_key(label: &str, org: Option<&str>, range: &DateRange) -> String {
    format!(
        "{label} {} {}_{}",
        org.unwrap_or(""),
        range.start,
        range.end
    )
}

/// A configured scope may carry an `org:`, `user:` or `author:` qualifier.
/// Chips show the bare name; the qualified value is what gets sent.
fn scope_name(scope: &str) -> &str {
    let trimmed = scope.trim_start();
    for prefix in ["org", "user", "author"] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            if let Some(name) = rest.trim_start().strip_prefix(':') {
                return name.trim_start();
            }
        }
    }
    scope
}

pub struct GithubView {
    range_filter: Entity<RangeFilter>,
    accounts: Vec<GithubAccount>,
    label: Option<String>,
    /// `None` is the account's "All orgs" view.
    org: Option<String>,
    range: DateRange,
    watched: bool,
    snaps: HashMap<String, Snapshot>,
    /// Views with a fetch in flight, each owning its own task. Only the newest
    /// request for a view may paint: replacing the task drops the older one.
    loading: HashMap<String, Task<()>>,
    /// Views whose last fetch failed. The cached snapshot stays on screen.
    failed: HashSet<String>,
    poll: Option<Task<()>>,
}

impl GithubView {
    pub fn new(range_filter: Entity<RangeFilter>, cx: &mut Context<Self>) -> Self {
        let range = range_filter.read(cx).range().clone();
        let mut view = GithubView {
            range_filter,
            accounts: Vec::new(),
            label: None,
            org: None,
            range,
            watched: false,
            snaps: HashMap::new(),
            loading: HashMap::new(),
            failed: HashSet::new(),
            poll: None,
        };
        view.reload_accounts(cx);
        view
    }

    /// Re-read the configured accounts: on startup and whenever they are edited
    /// on the Connectors page. Keeps the selection if it still exists.
    pub fn reload_accounts(&mut self, cx: &mut Context<Self>) {
        self.accounts = config::load().github.accounts;
        let still_there = self
            .label
            .as_ref()
            .is_some_and(|l| self.accounts.iter().any(|a| &a.label == l));
        if !still_there {
            self.label = self.accounts.first().map(|a| a.label.clone());
            self.org = None;
        }
        self.reconcile(cx);
    }

    /// Forget every cached view (a language change, or edited accounts).
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.snaps.clear();
        cx.notify();
    }

    pub fn set_watched(&mut self, watched: bool, cx: &mut Context<Self>) {
        if watched != self.watched {
            self.watched = watched;
            self.reconcile(cx);
        }
    }

    pub fn set_range(&mut self, range: DateRange, cx: &mut Context<Self>) {
        if range != self.range {
            self.range = range;
            self.reconcile(cx);
        }
    }

    /// The health of the selected view, for the sidebar dot.
    pub fn status(&self) -> Option<&Health> {
        self.snaps.get(&self.key()?).map(|s| &s.status)
    }

    fn key(&self) -> Option<String> {
        let label = self.label.as_ref()?;
        Some(view_key(label, self.org.as_deref(), &self.range))
    }

    fn select(&mut self, label: String, org: Option<String>, cx: &mut Context<Self>) {
        self.label = Some(label);
        self.org = org;
        self.reconcile(cx);
    }

    /// Keep the selected view fresh while the tab is watched: refetch if its
    /// snapshot is missing or older than the cadence, and poll - today only. A
    /// past day cannot change, and a multi-day range costs several paginated
    /// Search queries per org, which would eat GitHub's search budget.
    fn reconcile(&mut self, cx: &mut Context<Self>) {
        self.poll = None;
        cx.notify();
        if !self.watched {
            return;
        }
        let Some(key) = self.key() else { return };
        let fresh = self.snaps.get(&key).is_some_and(|s| {
            Utc::now()
                .signed_duration_since(s.fetched_at)
                .to_std()
                .is_ok_and(|age| age < REFRESH)
        });
        if !fresh {
            self.load(false, cx);
        }
        if self.range == DateRange::today() {
            self.poll = Some(cx.spawn(async move |this, cx| loop {
                cx.background_executor().timer(REFRESH).await;
                if this.update(cx, |this, cx| this.load(false, cx)).is_err() {
                    break;
                }
            }));
        }
    }

    /// Fetch the selected view in the background: never clears its cached
    /// snapshot, only flags it as loading and overlays the result. `force` is
    /// the Refresh button, which skips the connector's short reuse window.
    fn load(&mut self, force: bool, cx: &mut Context<Self>) {
        let (Some(label), Some(key)) = (self.label.clone(), self.key()) else {
            return;
        };
        let (org, range) = (self.org.clone(), self.range.clone());
        let job = runtime::spawn(fetch_account(label, org, range, force));
        let task_key = key.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                this.loading.remove(&task_key);
                match result {
                    Some(Ok(snapshot)) => {
                        this.failed.remove(&task_key);
                        this.snaps.insert(task_key, snapshot);
                    }
                    // A newer request took over: not a failure, it will paint.
                    Some(Err(e)) if e.contains(SUPERSEDED) => {}
                    _ => {
                        this.failed.insert(task_key);
                    }
                }
                cx.notify();
            });
        });
        self.loading.insert(key, task);
        cx.notify();
    }

    fn topbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let th = ui::theme(cx);
        let key = self.key();
        let loading = key.as_ref().is_some_and(|k| self.loading.contains_key(k));
        let failed = key.as_ref().is_some_and(|k| self.failed.contains(k));
        let snap = key.as_ref().and_then(|k| self.snaps.get(k));
        let status = if failed && !loading {
            Some(div().text_color(th.warn).child(t("app.refreshFailed")))
        } else {
            snap.map(|s| {
                div()
                    .text_color(th.muted)
                    .child(tf("app.updated", &[("time", &fetched_label(s))]))
            })
        };
        let button = div()
            .id("github-refresh")
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(12.))
            .py(px(4.))
            .bg(th.panel_2)
            .border_1()
            .border_color(th.border_bright)
            .text_color(th.text)
            .when(loading, |b| b.opacity(0.4).child(assets::spinner(th.text)))
            .when(!loading && self.label.is_some(), |b| {
                b.cursor_pointer()
                    .hover(move |s| s.bg(th.border).text_color(th.bright))
                    .on_click(cx.listener(|this, _, _, cx| this.load(true, cx)))
            })
            .child(t("app.refresh"));
        ui::topbar(
            "GitHub",
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
        .into_any_element()
    }
}

impl Render for GithubView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.accounts.is_empty() {
            return div()
                .flex()
                .flex_col()
                .size_full()
                .child(ui::topbar("GitHub", None, cx))
                .child(ui::empty(t("github.noAccounts"), cx));
        }

        let subtabs = ui::subtabs(cx).children(self.accounts.iter().map(|a| {
            let label = a.label.clone();
            ui::subtab(
                SharedString::from(format!("account-{label}")),
                label.clone(),
                self.label.as_deref() == Some(label.as_str()),
                cx,
            )
            .on_click(cx.listener(move |this, _, _, cx| this.select(label.clone(), None, cx)))
        }));

        let orgs = self
            .accounts
            .iter()
            .find(|a| Some(&a.label) == self.label.as_ref())
            .filter(|a| a.orgs.len() > 1)
            .map(|account| {
                let label = account.label.clone();
                div()
                    .flex()
                    .flex_none()
                    .flex_wrap()
                    .gap(px(6.))
                    .px(px(20.))
                    .pt(px(10.))
                    .child(
                        ui::chip("org-all", t("github.allOrgs"), self.org.is_none(), cx).on_click(
                            cx.listener({
                                let label = label.clone();
                                move |this, _, _, cx| this.select(label.clone(), None, cx)
                            }),
                        ),
                    )
                    .children(account.orgs.iter().map(|o| {
                        let (label, org) = (label.clone(), o.clone());
                        ui::chip(
                            SharedString::from(format!("org-{o}")),
                            scope_name(o).to_string(),
                            self.org.as_deref() == Some(o.as_str()),
                            cx,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.select(label.clone(), Some(org.clone()), cx)
                        }))
                    }))
            });

        let key = self.key();
        let loading = key.as_ref().is_some_and(|k| self.loading.contains_key(k));
        let body: Option<AnyElement> = match key.as_ref().and_then(|k| self.snaps.get(k)) {
            Some(snapshot) => {
                let dev_mode = prefs::get(cx).dev_mode;
                let scope = key.clone().unwrap_or_default();
                let banner = panels::status_banner(&snapshot.status, dev_mode, cx);
                let panels = panels::render_panels(snapshot, &scope, window, cx);
                Some(snapshot_view(banner, panels, scope.into()))
            }
            None if loading => Some(ui::empty(t("app.loading"), cx).into_any_element()),
            None => None,
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .child(self.topbar(cx))
            .child(subtabs)
            .children(orgs)
            .child(self.range_filter.clone())
            .children(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chips_show_the_bare_scope_name() {
        assert_eq!(scope_name("org:acme"), "acme");
        assert_eq!(scope_name("user: octocat"), "octocat");
        assert_eq!(scope_name(" author :ghost"), "ghost");
        assert_eq!(scope_name("acme"), "acme");
        // A name that merely starts with a qualifier word is left alone.
        assert_eq!(scope_name("organisation"), "organisation");
    }

    #[test]
    fn views_never_share_a_cache_key() {
        let r = DateRange::today();
        assert_ne!(
            view_key("work", None, &r),
            view_key("work", Some("acme"), &r)
        );
        assert_ne!(view_key("a", Some("b"), &r), view_key("a b", None, &r));
    }
}
