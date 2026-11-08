//! Claude connector setup: connect Anthropic Console with an Admin API key.
//!
//! There is no "Sign in with Claude" button, and that is not an omission:
//! Anthropic runs no third-party OAuth client registration, and reserves
//! subscription sign-in for Claude Code and claude.ai. The copy says so rather
//! than leaving the missing button looking unfinished.
//!
//! The key is verified against Anthropic before it is stored in the OS
//! keychain, and it is never read back. Only the organization name is
//! persisted to the config.

use fastdash_core::connectors::claude::{admin_api, CONSOLE_LABEL};
use fastdash_core::engine::secrets;
use gpui::{div, prelude::*, px, Context, Entity, EventEmitter, Subscription, Window};

use super::FormEvent;
use super::{account_card, account_head, patch_config, secret_field, stored_field, token_help};
use crate::i18n::{t, tf};
use crate::ui::text_input::{InputEvent, TextInput};
use crate::{runtime, ui};

/// Where the user provisions the key, opened in their real browser.
const ADMIN_KEYS_URL: &str = "https://platform.claude.com/settings/admin-keys";

pub struct ClaudeForm {
    stored: bool,
    org: String,
    key: Entity<TextInput>,
    stored_key: Entity<TextInput>,
    replacing: bool,
    connecting: bool,
    _submit: Subscription,
}

impl EventEmitter<FormEvent> for ClaudeForm {}

impl ClaudeForm {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let key = secret_field(t("settings.claudeKeyPlaceholder"), cx);
        let submit = cx.subscribe(&key, |this, _, event, cx| {
            if matches!(event, InputEvent::Submit) {
                this.connect(cx);
            }
        });
        let org = fastdash_core::engine::config::load().claude.console_org;
        cx.spawn(async move |this, cx| {
            let has = runtime::spawn_blocking(|| {
                matches!(secrets::get("claude", CONSOLE_LABEL), Ok(Some(_)))
            })
            .await
            .unwrap_or(false);
            let _ = this.update(cx, |this, cx| {
                this.stored = has;
                cx.notify();
            });
        })
        .detach();
        ClaudeForm {
            stored: false,
            org,
            key,
            stored_key: stored_field(cx),
            replacing: false,
            connecting: false,
            _submit: submit,
        }
    }

    fn connect(&mut self, cx: &mut Context<Self>) {
        if self.connecting {
            return;
        }
        let key = self.key.read(cx).text().trim().to_string();
        if key.is_empty() {
            cx.emit(FormEvent::Error(t("settings.claudeNeedKey").to_string()));
            return;
        }
        self.connecting = true;
        cx.notify();
        // Verify against Anthropic before anything is written to the keychain,
        // so a bad key fails here rather than as a broken dashboard later.
        let job = runtime::spawn(async move {
            let org = admin_api::verify_key(&key)
                .await
                .map_err(|e| e.to_string())?;
            secrets::set("claude", CONSOLE_LABEL, &key).map_err(|e| e.to_string())?;
            Ok::<_, String>(org.name)
        });
        cx.spawn(async move |this, cx| {
            let result = job.await.unwrap_or_else(|| Err("cancelled".into()));
            let _ = this.update(cx, |this, cx| {
                this.connecting = false;
                let saved = result.and_then(|org| {
                    patch_config(|c| c.claude.console_org = org.clone())?;
                    Ok(org)
                });
                match saved {
                    Ok(org) => {
                        this.org = org.clone();
                        this.stored = true;
                        this.replacing = false;
                        this.key.update(cx, |k, cx| k.set_text("", cx));
                        cx.emit(FormEvent::Saved);
                        cx.emit(FormEvent::Flash(tf(
                            "settings.claudeConnected",
                            &[("org", &org)],
                        )));
                    }
                    Err(e) => cx.emit(FormEvent::Error(e)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn disconnect(&mut self, cx: &mut Context<Self>) {
        let job = runtime::spawn_blocking(|| {
            secrets::delete("claude", CONSOLE_LABEL).map_err(|e| e.to_string())
        });
        cx.spawn(async move |this, cx| {
            let result = job.await.unwrap_or_else(|| Err("cancelled".into()));
            let _ = this.update(cx, |this, cx| {
                match result.and_then(|_| patch_config(|c| c.claude.console_org.clear())) {
                    Ok(_) => {
                        this.org.clear();
                        this.stored = false;
                        this.replacing = false;
                        cx.emit(FormEvent::Saved);
                        cx.emit(FormEvent::Flash(t("settings.claudeDisconnected")));
                    }
                    Err(e) => cx.emit(FormEvent::Error(e)),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for ClaudeForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let disconnect = self.stored.then(|| {
            ui::link_button("claude-disconnect", t("settings.claudeDisconnect"), cx)
                .on_click(cx.listener(|this, _, _, cx| this.disconnect(cx)))
                .into_any_element()
        });
        let lede = div()
            .mb(px(12.))
            .text_color(ui::theme(cx).muted)
            .child(t("settings.claudeLede"));

        let body = if self.stored && !self.replacing {
            let hint = if self.org.is_empty() {
                t("settings.claudeKeyStored")
            } else {
                tf("settings.claudeConnectedTo", &[("org", &self.org)])
            };
            let replace = ui::link_button("claude-replace", t("settings.replace"), cx).on_click(
                cx.listener(|this, _, window, cx| {
                    this.replacing = true;
                    this.key.update(cx, |k, cx| k.set_text("", cx));
                    this.key.read(cx).focus(window);
                    cx.notify();
                }),
            );
            div().child(
                ui::field(t("settings.claudeKey"), cx)
                    .child(super::stored_token(&self.stored_key, replace))
                    .child(ui::hint(hint, cx)),
            )
        } else {
            let connect = if self.connecting {
                ui::button_disabled("claude-connect", t("settings.connecting"), cx)
            } else {
                ui::button("claude-connect", t("settings.claudeConnect"), cx)
                    .on_click(cx.listener(|this, _, _, cx| this.connect(cx)))
            };
            let cancel = self.replacing.then(|| {
                ui::link_button("claude-cancel", t("settings.cancel"), cx).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.replacing = false;
                        this.key.update(cx, |k, cx| k.set_text("", cx));
                        cx.notify();
                    },
                ))
            });
            div()
                .child(
                    div().flex().mb(px(14.)).child(
                        ui::button("claude-console", t("settings.claudeOpenConsole"), cx)
                            .on_click(|_, _, _| ui::open_url(ADMIN_KEYS_URL)),
                    ),
                )
                .child(
                    ui::field(t("settings.claudeKey"), cx)
                        .child(self.key.clone())
                        .child(token_help(
                            vec![t("settings.claudeKeySteps"), t("settings.claudeKeyScope")],
                            cx,
                        )),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(16.))
                        .mt(px(12.))
                        .child(connect)
                        .children(cancel),
                )
        };

        ui::card(cx)
            .child(
                account_card(cx)
                    .child(account_head(t("settings.claudeConsole"), disconnect, cx))
                    .child(lede)
                    .child(body),
            )
            .child(token_help(
                vec![t("settings.claudeNoOauth"), t("settings.claudePlanMeters")],
                cx,
            ))
    }
}
