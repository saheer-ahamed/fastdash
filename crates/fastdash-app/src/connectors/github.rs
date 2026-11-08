//! GitHub connector setup: accounts, each with a token (Device Flow sign-in or
//! a pasted PAT) and the orgs to track. Tokens go straight to the OS keychain
//! and are never read back; only labels and orgs are persisted to the config,
//! and only the `github` slice of it.

use std::collections::HashMap;

use fastdash_core::connectors::github::device_flow::{self, DeviceCode};
use fastdash_core::engine::config::GithubAccount;
use fastdash_core::engine::secrets;
use gpui::{
    div, prelude::*, px, ClipboardItem, Context, Entity, EventEmitter, FontWeight, Subscription,
    Task, Window,
};

use super::{
    account_card, account_head, patch_config, secret_field, stored_field, stored_token, text_field,
    token_guide, token_help, FormEvent,
};
use crate::i18n::{t, tf};
use crate::runtime;
use crate::ui::text_input::{InputEvent, TextInput};
use crate::ui::{self, overlay};

/// The README section spelling out the scopes each panel needs.
pub const TOKEN_DOCS_URL: &str = "https://github.com/saheer-ahamed/fastdash#connecting-github";

/// A scope and its qualifier are one entry even when typed as `user: octocat`,
/// so collapse around the colon before splitting on commas and whitespace.
fn parse_scopes(raw: &str) -> Vec<String> {
    let mut collapsed = String::with_capacity(raw.len());
    for part in raw.split(':') {
        if !collapsed.is_empty() {
            collapsed = collapsed.trim_end().to_string();
            collapsed.push(':');
            collapsed.push_str(part.trim_start());
        } else {
            collapsed.push_str(part);
        }
    }
    super::parse_list(&collapsed)
}

/// One editable account. `key` is stable so per-row state survives removals.
struct Row {
    key: usize,
    label: Entity<TextInput>,
    orgs: Entity<TextInput>,
    token: Entity<TextInput>,
    stored_token: Entity<TextInput>,
    replacing: bool,
}

/// The one Device Flow sign-in in flight, tagged with its row.
struct Connect {
    key: usize,
    device: Option<DeviceCode>,
    copied: bool,
    _task: Task<()>,
}

pub struct GithubForm {
    rows: Vec<Row>,
    next_key: usize,
    /// Whether a token exists in the keychain, per account label.
    stored: HashMap<String, bool>,
    connect: Option<Connect>,
    guide_open: bool,
    guide_focus: gpui::FocusHandle,
    _copied: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<FormEvent> for GithubForm {}

impl GithubForm {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut form = GithubForm {
            rows: Vec::new(),
            next_key: 0,
            stored: HashMap::new(),
            connect: None,
            guide_open: false,
            guide_focus: cx.focus_handle(),
            _copied: None,
            _subscriptions: Vec::new(),
        };
        for account in fastdash_core::engine::config::load().github.accounts {
            form.push_row(&account.label, &account.orgs.join(", "), cx);
        }
        form.refresh_stored(cx);
        form
    }

    fn push_row(&mut self, label: &str, orgs: &str, cx: &mut Context<Self>) {
        let label = text_field(label.to_string(), "", cx);
        let orgs = text_field(orgs.to_string(), t("settings.orgsPlaceholder"), cx);
        let token = secret_field(t("settings.tokenPlaceholder"), cx);
        // A renamed account may already have a token stored under the name.
        self._subscriptions
            .push(cx.subscribe(&label, |this, _, event, cx| {
                if matches!(event, InputEvent::Changed) {
                    this.refresh_stored(cx);
                }
            }));
        self.rows.push(Row {
            key: self.next_key,
            label,
            orgs,
            token,
            stored_token: stored_field(cx),
            replacing: false,
        });
        self.next_key += 1;
        cx.notify();
    }

    fn label_of(&self, row: &Row, cx: &Context<Self>) -> String {
        row.label.read(cx).text().trim().to_string()
    }

    /// Ask the keychain which labels have a token - never what the token is.
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
                    let has = matches!(secrets::get("github", &l), Ok(Some(_)));
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

    /// Rows to persisted accounts, dropping any without a label.
    fn accounts(&self, cx: &Context<Self>) -> Vec<GithubAccount> {
        self.rows
            .iter()
            .map(|r| GithubAccount {
                label: self.label_of(r, cx),
                orgs: parse_scopes(r.orgs.read(cx).text()),
            })
            .filter(|a| !a.label.is_empty())
            .collect()
    }

    fn saved(&self, cx: &mut Context<Self>) {
        cx.emit(FormEvent::Saved);
        cx.emit(FormEvent::Flash(tf(
            "settings.saved",
            &[("section", &t("settings.github"))],
        )));
    }

    fn save_all(&mut self, cx: &mut Context<Self>) {
        // Freshly typed tokens go to the keychain first, keyed by label.
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
                secrets::set("github", label, token).map_err(|e| e.to_string())?;
            }
            patch_config(|c| c.github.accounts = accounts)?;
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
                let _ = secrets::delete("github", &label);
            }
            patch_config(|c| c.github.accounts = accounts).map(|_| ())
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

    /// Device Flow for one row: fetch a code, show it while the user approves
    /// in the browser, then store the token and persist the account.
    fn start_connect(&mut self, key: usize, cx: &mut Context<Self>) {
        let Some(row) = self.rows.iter().find(|r| r.key == key) else {
            return;
        };
        let label = self.label_of(row, cx);
        if label.is_empty() {
            cx.emit(FormEvent::Error("Set an account label first".into()));
            return;
        }
        let task = cx.spawn(async move |this, cx| {
            let started = runtime::spawn(device_flow::start()).await;
            let device = match started {
                Some(Ok(device)) => device,
                other => {
                    let err = match other {
                        Some(Err(e)) => e,
                        _ => "cancelled".into(),
                    };
                    let _ = this.update(cx, |this, cx| {
                        this.connect = None;
                        cx.emit(FormEvent::Error(err));
                        cx.notify();
                    });
                    return;
                }
            };
            let (code, interval) = (device.device_code.clone(), device.interval);
            let _ = this.update(cx, |this, cx| {
                if let Some(c) = this.connect.as_mut() {
                    c.device = Some(device);
                }
                cx.notify();
            });
            // Aborted with this task when the user cancels.
            let poll_label = label.clone();
            let polled = runtime::spawn_abortable(async move {
                let token = device_flow::poll(&code, interval).await?;
                let login = device_flow::fetch_login(&token).await?;
                secrets::set("github", &poll_label, &token).map_err(|e| e.to_string())?;
                Ok::<_, String>(login)
            })
            .await
            .unwrap_or_else(|| Err("cancelled".into()));
            let _ = this.update(cx, |this, cx| {
                this.connect = None;
                match polled {
                    Ok(login) => {
                        this.stored.insert(label.clone(), true);
                        if let Some(row) = this.rows.iter_mut().find(|r| r.key == key) {
                            row.replacing = false;
                        }
                        let accounts = this.accounts(cx);
                        match patch_config(|c| c.github.accounts = accounts) {
                            Ok(_) => {
                                cx.emit(FormEvent::Saved);
                                cx.emit(FormEvent::Flash(tf(
                                    "settings.connectedAs",
                                    &[("login", &login)],
                                )));
                            }
                            Err(e) => cx.emit(FormEvent::Error(e)),
                        }
                    }
                    Err(e) => cx.emit(FormEvent::Error(e)),
                }
                cx.notify();
            });
        });
        self.connect = Some(Connect {
            key,
            device: None,
            copied: false,
            _task: task,
        });
        cx.notify();
    }

    fn copy_code(&mut self, cx: &mut Context<Self>) {
        let Some(code) = self
            .connect
            .as_ref()
            .and_then(|c| c.device.as_ref())
            .map(|d| d.user_code.clone())
        else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(code));
        if let Some(c) = self.connect.as_mut() {
            c.copied = true;
        }
        self._copied = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(1500))
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(c) = this.connect.as_mut() {
                    c.copied = false;
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn device_panel(&self, device: &DeviceCode, copied: bool, cx: &mut Context<Self>) -> gpui::Div {
        let th = ui::theme(cx);
        let uri = device.verification_uri.clone();
        div()
            .flex()
            .flex_col()
            .border_1()
            .border_color(th.border_bright)
            .bg(th.bg)
            .p(px(14.))
            .mb(px(12.))
            .child(
                div()
                    .mb(px(10.))
                    .text_color(th.text)
                    .child(t("settings.deviceInstructions")),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(14.))
                    .mb(px(10.))
                    .child(
                        div()
                            .px(px(14.))
                            .py(px(6.))
                            .bg(th.panel_2)
                            .border_1()
                            .border_color(th.border_bright)
                            .text_size(px(24.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(th.bright)
                            .flex()
                            // The web UI's 4px letter-spacing, glyph by glyph.
                            .gap(px(4.))
                            .children(device.user_code.chars().map(|c| div().child(c.to_string()))),
                    )
                    .child(
                        ui::link_button(
                            "device-copy",
                            if copied {
                                t("settings.copied")
                            } else {
                                t("settings.copy")
                            },
                            cx,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.copy_code(cx))),
                    ),
            )
            .child(
                div()
                    .mb(px(10.))
                    .text_color(th.muted)
                    .child(t("settings.deviceWaiting")),
            )
            .child(
                div()
                    .flex()
                    .gap(px(16.))
                    .child(
                        ui::link_button("device-open", t("settings.deviceOpen"), cx)
                            .on_click(move |_, _, _| ui::open_url(&uri)),
                    )
                    .child(
                        ui::link_button("device-cancel", t("settings.cancel"), cx).on_click(
                            cx.listener(|this, _, _, cx| {
                                // Dropping the task aborts the poll.
                                this.connect = None;
                                cx.notify();
                            }),
                        ),
                    ),
            )
    }

    fn row_view(&self, i: usize, row: &Row, cx: &mut Context<Self>) -> gpui::Div {
        let th = ui::theme(cx);
        let key = row.key;
        let label = self.label_of(row, cx);
        let token_stored = !label.is_empty() && self.stored.get(&label).copied().unwrap_or(false);
        let connecting = self.connect.as_ref().is_some_and(|c| c.key == key);
        let device = self
            .connect
            .as_ref()
            .filter(|c| c.key == key)
            .and_then(|c| c.device.as_ref().map(|d| (d.clone(), c.copied)));

        let remove = ui::link_button(("gh-remove", key), t("settings.removeAccount"), cx)
            .on_click(cx.listener(move |this, _, _, cx| this.remove(key, cx)))
            .into_any_element();

        let token_area = match device {
            Some((device, copied)) => self.device_panel(&device, copied, cx),
            None => {
                let connect = if connecting {
                    ui::button_disabled(("gh-connect", key), t("settings.connecting"), cx)
                } else {
                    ui::button(("gh-connect", key), t("settings.connectGithub"), cx)
                        .on_click(cx.listener(move |this, _, _, cx| this.start_connect(key, cx)))
                };
                let label_text = if token_stored {
                    t("settings.orPasteToken")
                } else {
                    t("settings.pat")
                };
                // The (i) opens the step-by-step guide. A click, not a hover:
                // it is read with GitHub open in another window.
                let info = div()
                    .id(("gh-info", key))
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(16.))
                    .border_1()
                    .border_color(th.border_bright)
                    .text_size(px(11.))
                    .line_height(px(11.))
                    .text_color(th.muted)
                    .cursor_pointer()
                    .hover(move |s| s.text_color(th.bright).border_color(th.fill))
                    .tooltip(ui::tooltip::text(t("patGuide.open")))
                    .child("i")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.guide_open = true;
                        window.focus(&this.guide_focus);
                        cx.notify();
                    }));
                let field = if token_stored && !row.replacing {
                    let replace = ui::link_button(("gh-replace", key), t("settings.replace"), cx)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if let Some(row) = this.rows.iter_mut().find(|r| r.key == key) {
                                row.replacing = true;
                                row.token.update(cx, |i, cx| i.set_text("", cx));
                                row.token.read(cx).focus(window);
                            }
                            cx.notify();
                        }));
                    stored_token(&row.stored_token, replace)
                } else {
                    div().child(row.token.clone())
                };
                let help = (!token_stored || row.replacing).then(|| {
                    token_help(
                        vec![t("settings.patScopes"), t("settings.patFineGrained")],
                        cx,
                    )
                    .child(
                        ui::link_button(("gh-docs", key), t("settings.patDocs"), cx)
                            .on_click(|_, _, _| ui::open_url(TOKEN_DOCS_URL)),
                    )
                });
                div().child(div().flex().mb(px(14.)).child(connect)).child(
                    div()
                        .flex()
                        .flex_col()
                        .mb(px(12.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .mb(px(5.))
                                .child(div().text_color(th.muted).child(label_text))
                                .child(info),
                        )
                        .child(field)
                        .children(token_stored.then(|| ui::hint(t("settings.tokenStored"), cx)))
                        .children(help),
                )
            }
        };

        account_card(cx)
            .child(account_head(
                tf("settings.accountN", &[("n", &(i + 1).to_string())]),
                Some(remove),
                cx,
            ))
            .child(ui::field(t("settings.accountLabel"), cx).child(row.label.clone()))
            .child(token_area)
            .child(
                ui::field(t("settings.orgs"), cx)
                    .mb_0()
                    .child(row.orgs.clone()),
            )
    }
}

impl Render for GithubForm {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                .child(t("settings.noGithubAccounts"))
        });
        let guide = self.guide_open.then(|| {
            let this = cx.entity().downgrade();
            let close = move |_: &mut Window, cx: &mut gpui::App| {
                let _ = this.update(cx, |this, cx| {
                    this.guide_open = false;
                    cx.notify();
                });
            };
            let body = token_guide::render(TOKEN_DOCS_URL, close.clone(), cx);
            overlay::modal(window, &self.guide_focus, body, close)
        });
        ui::card(cx)
            .children(empty)
            .children(rows)
            .child(
                div()
                    .flex()
                    .gap(px(12.))
                    .child(
                        ui::button("gh-add", t("settings.addAccount"), cx)
                            .on_click(cx.listener(|this, _, _, cx| this.push_row("", "", cx))),
                    )
                    .child(
                        ui::button("gh-save", t("settings.save"), cx)
                            .on_click(cx.listener(|this, _, _, cx| this.save_all(cx))),
                    ),
            )
            .children(guide)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_qualifier_and_its_name_stay_one_entry() {
        assert_eq!(
            parse_scopes("acme, user: octocat author :me"),
            vec!["acme", "user:octocat", "author:me"]
        );
        assert_eq!(
            parse_scopes("org:acme,org:beta"),
            vec!["org:acme", "org:beta"]
        );
    }
}
