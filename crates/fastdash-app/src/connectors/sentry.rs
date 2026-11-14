//! Sentry connector setup: connections, each with an auth token, the Sentry
//! origin (SaaS, a region host, or self-hosted), and the organizations to
//! report on. Tokens go straight to the OS keychain and are never read back;
//! only labels, URLs and slugs are persisted, and only the `sentry` slice.

use std::collections::HashMap;

use fastdash_core::engine::config::SentryAccount;
use fastdash_core::engine::secrets;
use gpui::{div, prelude::*, px, Context, Entity, EventEmitter, Subscription, Window};

use super::{
    account_card, account_head, parse_list, patch_config, secret_field, stored_field, stored_token,
    text_field, token_help, FormEvent,
};
use crate::i18n::{t, tf};
use crate::runtime;
use crate::ui;
use crate::ui::text_input::{InputEvent, TextInput};

/// The README section spelling out which token kind and scopes are needed.
const TOKEN_DOCS_URL: &str = "https://github.com/saheer-ahamed/fastdash#connecting-sentry";

struct Row {
    key: usize,
    label: Entity<TextInput>,
    base_url: Entity<TextInput>,
    orgs: Entity<TextInput>,
    token: Entity<TextInput>,
    stored_token: Entity<TextInput>,
    replacing: bool,
}

pub struct SentryForm {
    rows: Vec<Row>,
    next_key: usize,
    stored: HashMap<String, bool>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<FormEvent> for SentryForm {}

impl SentryForm {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut form = SentryForm {
            rows: Vec::new(),
            next_key: 0,
            stored: HashMap::new(),
            _subscriptions: Vec::new(),
        };
        for account in fastdash_core::engine::config::load().sentry.accounts {
            form.push_row(
                &account.label,
                &account.base_url,
                &account.organizations.join(", "),
                cx,
            );
        }
        form.refresh_stored(cx);
        form
    }

    fn push_row(&mut self, label: &str, base_url: &str, orgs: &str, cx: &mut Context<Self>) {
        let label = text_field(label.to_string(), "", cx);
        self._subscriptions
            .push(cx.subscribe(&label, |this, _, event, cx| {
                if matches!(event, InputEvent::Changed) {
                    this.refresh_stored(cx);
                }
            }));
        self.rows.push(Row {
            key: self.next_key,
            label,
            base_url: text_field(
                base_url.to_string(),
                t("sentry.setup.baseUrlPlaceholder"),
                cx,
            ),
            orgs: text_field(orgs.to_string(), t("sentry.setup.orgsPlaceholder"), cx),
            token: secret_field(t("settings.tokenPlaceholder"), cx),
            stored_token: stored_field(cx),
            replacing: false,
        });
        self.next_key += 1;
        cx.notify();
    }

    fn label_of(&self, row: &Row, cx: &Context<Self>) -> String {
        row.label.read(cx).text().trim().to_string()
    }

    fn refresh_stored(&mut self, cx: &mut Context<Self>) {
        let labels: Vec<String> = self
            .rows
            .iter()
            .map(|r| self.label_of(r, cx))
            .filter(|l| !l.is_empty())
            .collect();
        let job = runtime::spawn_blocking(move || {
            labels
                .into_iter()
                .map(|l| {
                    let has = {
                        #[cfg(target_arch = "wasm32")]
                        {
                            let _ = &l;
                            fastdash_core::engine::web_session::has_sentry()
                        }
                        #[cfg(not(target_arch = "wasm32"))]
                        {
                            matches!(secrets::get("sentry", &l), Ok(Some(_)))
                        }
                    };
                    (l, has)
                })
                .collect::<HashMap<_, _>>()
        });
        cx.spawn(async move |this, cx| {
            if let Some(stored) = job.await {
                let _ = this.update(cx, |this, cx| {
                    this.stored = stored;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn accounts(&self, cx: &Context<Self>) -> Vec<SentryAccount> {
        self.rows
            .iter()
            .map(|r| SentryAccount {
                label: self.label_of(r, cx),
                base_url: r.base_url.read(cx).text().trim().to_string(),
                organizations: parse_list(r.orgs.read(cx).text()),
            })
            .filter(|a| !a.label.is_empty())
            .collect()
    }

    fn saved(&self, cx: &mut Context<Self>) {
        cx.emit(FormEvent::Saved);
        cx.emit(FormEvent::Flash(tf(
            "settings.saved",
            &[("section", &t("settings.sentry"))],
        )));
    }

    fn save_all(&mut self, cx: &mut Context<Self>) {
        #[cfg(target_arch = "wasm32")]
        if self
            .rows
            .iter()
            .any(|r| !r.token.read(cx).text().trim().is_empty())
        {
            cx.emit(FormEvent::Error(t("web.useSignIn").to_string()));
            return;
        }
        let tokens: Vec<(String, String)> = self
            .rows
            .iter()
            .map(|r| {
                (
                    self.label_of(r, cx),
                    r.token.read(cx).text().trim().to_string(),
                )
            })
            .filter(|(l, tok)| !l.is_empty() && !tok.is_empty())
            .collect();
        let accounts = self.accounts(cx);
        let job = runtime::spawn_blocking(move || {
            for (label, token) in &tokens {
                secrets::set("sentry", label, token).map_err(|e| e.to_string())?;
            }
            patch_config(|c| c.sentry.accounts = accounts)?;
            Ok::<_, String>(tokens.into_iter().map(|(l, _)| l).collect::<Vec<_>>())
        });
        cx.spawn(async move |this, cx| {
            let result = job.await.unwrap_or_else(|| Err("cancelled".into()));
            let _ = this.update(cx, |this, cx| match result {
                Ok(stored) => {
                    for label in stored {
                        this.stored.insert(label, true);
                    }
                    for row in &mut this.rows {
                        row.replacing = false;
                        row.token.update(cx, |i, cx| i.set_text("", cx));
                    }
                    this.saved(cx);
                    cx.notify();
                }
                Err(e) => cx.emit(FormEvent::Error(e)),
            });
        })
        .detach();
    }

    fn remove(&mut self, key: usize, cx: &mut Context<Self>) {
        let Some(ix) = self.rows.iter().position(|r| r.key == key) else {
            return;
        };
        let label = self.label_of(&self.rows[ix], cx);
        self.rows.remove(ix);
        let accounts = self.accounts(cx);
        let job = runtime::spawn_blocking(move || {
            if !label.is_empty() {
                let _ = secrets::delete("sentry", &label);
            }
            patch_config(|c| c.sentry.accounts = accounts).map(|_| ())
        });
        cx.spawn(async move |this, cx| {
            let result = job.await.unwrap_or_else(|| Err("cancelled".into()));
            let _ = this.update(cx, |this, cx| match result {
                Ok(()) => this.saved(cx),
                Err(e) => cx.emit(FormEvent::Error(e)),
            });
        })
        .detach();
        cx.notify();
    }

    fn row_view(&self, i: usize, row: &Row, cx: &mut Context<Self>) -> gpui::Div {
        let key = row.key;
        let label = self.label_of(row, cx);
        let token_stored = !label.is_empty() && self.stored.get(&label).copied().unwrap_or(false);
        let remove = ui::link_button(("sentry-remove", key), t("settings.removeAccount"), cx)
            .on_click(cx.listener(move |this, _, _, cx| this.remove(key, cx)))
            .into_any_element();

        let token = if token_stored && !row.replacing {
            let replace = ui::link_button(("sentry-replace", key), t("settings.replace"), cx)
                .on_click(cx.listener(move |this, _, window, cx| {
                    if let Some(row) = this.rows.iter_mut().find(|r| r.key == key) {
                        row.replacing = true;
                        row.token.update(cx, |i, cx| i.set_text("", cx));
                        let handle = row.token.read(cx).handle();
                        window.focus(&handle, cx);
                    }
                    cx.notify();
                }));
            stored_token(&row.stored_token, replace)
        } else {
            div().child(row.token.clone())
        };
        // Which scopes to tick, at the moment the token is being made - and
        // which token kind is a dead end.
        let help = (!token_stored || row.replacing).then(|| {
            token_help(
                vec![t("sentry.setup.tokenHelp"), t("sentry.setup.tokenWarning")],
                cx,
            )
            .child(
                ui::link_button(("sentry-docs", key), t("sentry.setup.docs"), cx)
                    .on_click(|_, _, _| ui::open_url(TOKEN_DOCS_URL)),
            )
        });

        account_card(cx)
            .child(account_head(
                tf("settings.accountN", &[("n", &(i + 1).to_string())]),
                Some(remove),
                cx,
            ))
            .child(ui::field(t("settings.accountLabel"), cx).child(row.label.clone()))
            .child(
                ui::field(t("sentry.setup.token"), cx)
                    .child(token)
                    .children(token_stored.then(|| ui::hint(t("settings.tokenStored"), cx)))
                    .children(help),
            )
            .child(
                ui::field(t("sentry.setup.baseUrl"), cx)
                    .child(row.base_url.clone())
                    .child(ui::hint(t("sentry.setup.baseUrlHelp"), cx)),
            )
            .child(
                ui::field(t("sentry.setup.orgs"), cx)
                    .mb_0()
                    .child(row.orgs.clone())
                    .child(ui::hint(t("sentry.setup.orgsHelp"), cx)),
            )
    }
}

impl Render for SentryForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows: Vec<_> = self
            .rows
            .iter()
            .enumerate()
            .map(|(i, row)| self.row_view(i, row, cx))
            .collect();
        let empty = self.rows.is_empty().then(|| {
            div()
                .mb(px(12.))
                .text_color(ui::theme(cx).muted)
                .child(t("sentry.setup.noAccounts"))
        });
        ui::card(cx).children(empty).children(rows).child(
            div()
                .flex()
                .gap(px(12.))
                .child(
                    ui::button("sentry-add", t("settings.addAccount"), cx)
                        .on_click(cx.listener(|this, _, _, cx| this.push_row("", "", "", cx))),
                )
                .child(
                    ui::button("sentry-save", t("settings.save"), cx)
                        .on_click(cx.listener(|this, _, _, cx| this.save_all(cx))),
                ),
        )
    }
}
