//! The GitHub dashboard: one sub-tab per connected account, with an org filter
//! (All + each org) inside the account.
//!
//! Each (account, org, range) view fetches through the connector's own
//! `fetch_account`, which cancels whatever GitHub fetch was still in flight -
//! flipping through sub-tabs costs one fetch, not one per click. A view saved
//! by a previous session is painted immediately and reconciled in the
//! background. Once it has been loaded this session, leaving and coming back
//! just shows that copy until Refresh.

use std::collections::{HashMap, HashSet};

use std::time::Instant;

use fastdash_core::connectors::github::{self, fetch_account, SUPERSEDED};
use fastdash_core::engine::config::{self, GithubAccount};
use fastdash_core::engine::connector::{Health, Snapshot};
use fastdash_core::engine::perf::{self, CacheState};
use fastdash_core::engine::range::DateRange;
use gpui::{div, prelude::*, px, AnyElement, Context, Entity, SharedString, Task, Window};

use crate::dashboard::{fetched_label, snapshot_view};
use crate::i18n::{t, tf};
use crate::range_filter::RangeFilter;
use crate::{assets, panels, prefs, runtime, ui};

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
    /// The fetch in flight for a view, tagged so a late result cannot paint
    /// over the request that replaced it.
    loading: HashMap<String, (u64, Task<()>)>,
    load_gen: u64,
    /// Views whose last fetch failed. The cached snapshot stays on screen.
    failed: HashSet<String>,
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
            load_gen: 0,
            failed: HashSet::new(),
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
    /// Rendered panels are dropped; the search cache in the connector stays
    /// and is revalidated with one probe per scope on the next fetch.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.snaps.clear();
        self.loading.clear();
        github::forget_cached_views();
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

    /// Fetch the selected view the first time it is opened this session.
    /// Switching back to a view that was already loaded does not refetch;
    /// Refresh does. A view saved by a previous session is painted first, and
    /// the fetch reconciles it in the background.
    fn reconcile(&mut self, cx: &mut Context<Self>) {
        cx.notify();
        if !self.watched {
            // Leaving the tab cancels whatever GitHub fetch is in flight so it
            // stops spending the Search budget on a view nobody is looking at.
            self.loading.clear();
            return;
        }
        let Some(key) = self.key() else { return };
        let opened_this_session = self.snaps.contains_key(&key);
        if !opened_this_session {
            self.paint_saved(&key);
        }
        if opened_this_session {
            // This view is ready. Drop a fetch for a view the user just left,
            // but keep a refresh of the view that is on screen.
            self.loading.retain(|k, _| k == &key);
            return;
        }
        if self.loading.contains_key(&key) {
            return;
        }
        self.load(false, cx);
    }

    /// Put the last saved render of `key` on screen. No network, so the
    /// dashboard is up before the reconcile fetch has left the machine.
    fn paint_saved(&mut self, key: &str) {
        let Some(label) = self.label.clone() else {
            return;
        };
        let started = Instant::now();
        let Some(snapshot) = github::cached_view(&label, self.org.as_deref(), &self.range) else {
            return;
        };
        perf::record("github.paint", started.elapsed(), 0, CacheState::Hit, true);
        self.snaps.insert(key.to_string(), snapshot);
    }

    /// Fetch the selected view in the background: never clears its cached
    /// snapshot, only flags it as loading and overlays the result. `force` is
    /// the Refresh button, which skips the connector's short reuse window.
    fn load(&mut self, force: bool, cx: &mut Context<Self>) {
        let (Some(label), Some(key)) = (self.label.clone(), self.key()) else {
            return;
        };
        let (org, range) = (self.org.clone(), self.range.clone());
        // One GitHub fetch at a time: the connector cancels the previous
        // request, and dropping these tasks aborts it if the user has moved on.
        self.loading.clear();
        self.load_gen = self.load_gen.wrapping_add(1);
        let gen = self.load_gen;
        let job = runtime::spawn_abortable(fetch_account(label, org, range, force));
        let task_key = key.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                let current = this.loading.get(&task_key).map(|(id, _)| *id);
                if current != Some(gen) {
                    return;
                }
                this.loading.remove(&task_key);
                match result {
                    Some(Ok(snapshot)) => {
                        let painted_ok = matches!(
                            this.snaps.get(&task_key).map(|s| &s.status),
                            Some(Health::Ok)
                        );
                        let soft_failure = matches!(
                            snapshot.status,
                            Health::Error { .. } | Health::RateLimited { .. }
                        );
                        if painted_ok && soft_failure {
                            // The reconcile failed. Leave the saved dashboard
                            // on screen and say the refresh did not land.
                            this.failed.insert(task_key);
                        } else {
                            this.failed.remove(&task_key);
                            this.snaps.insert(task_key, snapshot);
                        }
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
        self.loading.insert(key, (gen, task));
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
        let _span = fastdash_core::engine::perf::span("ui.github");
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
