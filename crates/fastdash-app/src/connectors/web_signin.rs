//! Browser sign-in. Tokens are posted to the Worker once and are not kept
//! in the page. Later connector calls ride the session cookie.

use fastdash_core::engine::config::{self, GithubAccount, SentryAccount};
use fastdash_core::engine::web_session::{self, WebSession};
use gpui::{div, prelude::*, px, Context, Entity, EventEmitter, SharedString};
use serde::Deserialize;

use super::secret_field;
use crate::i18n::t;
use crate::runtime;
use crate::ui;
use crate::ui::text_input::TextInput;

/// The session changed. The shell re-reads which connectors are configured.
pub struct Changed;

pub struct WebSignIn {
    github: Entity<TextInput>,
    anthropic: Entity<TextInput>,
    sentry: Entity<TextInput>,
    sentry_host: Entity<TextInput>,
    busy: bool,
    signed_in: bool,
    login: Option<String>,
    note: Option<(bool, SharedString)>,
}

impl EventEmitter<Changed> for WebSignIn {}

impl WebSignIn {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let github = secret_field(t("web.githubPlaceholder"), cx);
        let anthropic = secret_field(t("web.anthropicPlaceholder"), cx);
        let sentry = secret_field(t("web.sentryPlaceholder"), cx);
        let sentry_host =
            cx.new(|cx| TextInput::new(cx).placeholder(t("web.sentryHostPlaceholder").to_string()));
        cx.spawn(async move |this, cx| {
            let status = match runtime::spawn(session_status()).await {
                Some(Ok(status)) => Some(status),
                _ => None,
            };
            let _ = this.update(cx, |this, cx| {
                if let Some(status) = status {
                    this.signed_in = status.connected();
                    this.login = status.login.clone();
                    apply_status(&status);
                    cx.emit(Changed);
                }
                cx.notify();
            });
        })
        .detach();
        Self {
            github,
            anthropic,
            sentry,
            sentry_host,
            busy: false,
            signed_in: false,
            login: None,
            note: None,
        }
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let github = self.github.read(cx).text().trim().to_string();
        let anthropic = self.anthropic.read(cx).text().trim().to_string();
        let sentry = self.sentry.read(cx).text().trim().to_string();
        let host = self.sentry_host.read(cx).text().trim().to_string();
        if github.is_empty() && anthropic.is_empty() && sentry.is_empty() {
            self.note = Some((true, t("web.none")));
            cx.notify();
            return;
        }
        self.busy = true;
        self.note = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = runtime::spawn(create_session(github, anthropic, sentry, host)).await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Some(Ok(status)) => {
                        this.signed_in = true;
                        this.login = status.login.clone();
                        apply_status(&status);
                        this.github.update(cx, |input, cx| input.set_text("", cx));
                        this.anthropic
                            .update(cx, |input, cx| input.set_text("", cx));
                        this.sentry.update(cx, |input, cx| input.set_text("", cx));
                        this.note = Some((false, t("web.saved")));
                        cx.emit(Changed);
                    }
                    Some(Err(err)) => this.note = Some((true, err.into())),
                    None => this.note = Some((true, t("web.failed"))),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn sign_out(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = runtime::spawn(delete_session()).await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Some(Ok(())) => {
                        this.signed_in = false;
                        this.login = None;
                        web_session::clear();
                        this.note = Some((false, t("web.signedOut")));
                        cx.emit(Changed);
                    }
                    Some(Err(err)) => this.note = Some((true, err.into())),
                    None => this.note = Some((true, t("web.failed"))),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for WebSignIn {
    fn render(&mut self, _window: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = ui::theme(cx);
        let status = if self.signed_in {
            match &self.login {
                Some(login) if !login.is_empty() => format!("{} ({login})", t("web.signedIn")),
                _ => t("web.signedIn").to_string(),
            }
        } else {
            t("web.signedOutIdle").to_string()
        };
        let submit_label = if self.busy {
            t("web.working")
        } else {
            t("web.submit")
        };
        ui::card(cx)
            .child(ui::card_title(t("web.title"), cx))
            .child(
                div()
                    .mb(px(12.))
                    .text_color(th.muted)
                    .line_height(gpui::relative(1.5))
                    .child(t("web.lede")),
            )
            .child(div().mb(px(8.)).text_color(th.bright).child(status))
            .child(ui::field(t("web.github"), cx).child(self.github.clone()))
            .child(ui::field(t("web.anthropic"), cx).child(self.anthropic.clone()))
            .child(ui::field(t("web.sentry"), cx).child(self.sentry.clone()))
            .child(ui::field(t("web.sentryHost"), cx).child(self.sentry_host.clone()))
            .child(
                div()
                    .flex()
                    .gap(px(12.))
                    .mt(px(8.))
                    .child(
                        ui::button("web-sign-in", submit_label, cx)
                            .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
                    )
                    .when(self.signed_in, |row| {
                        row.child(
                            ui::button("web-sign-out", t("web.signOut"), cx)
                                .on_click(cx.listener(|this, _, _, cx| this.sign_out(cx))),
                        )
                    }),
            )
            .children(self.note.as_ref().map(|(error, text)| {
                let color = if *error { th.err } else { th.muted };
                div().mt(px(8.)).text_color(color).child(text.clone())
            }))
    }
}

struct Status {
    github: bool,
    anthropic: bool,
    sentry: bool,
    login: Option<String>,
    sentry_host: Option<String>,
}

impl Status {
    fn connected(&self) -> bool {
        self.github || self.anthropic || self.sentry
    }
}

fn apply_status(status: &Status) {
    let mut cfg = config::load();
    if status.github && cfg.github.accounts.is_empty() {
        cfg.github.accounts.push(GithubAccount {
            label: status
                .login
                .clone()
                .filter(|login| !login.is_empty())
                .unwrap_or_else(|| "github".to_string()),
            orgs: Vec::new(),
        });
    }
    if status.sentry {
        let base = status
            .sentry_host
            .as_ref()
            .filter(|host| !host.is_empty())
            .map(|host| format!("https://{host}"))
            .unwrap_or_default();
        if let Some(account) = cfg.sentry.accounts.first_mut() {
            if account.base_url.is_empty() && !base.is_empty() {
                account.base_url = base;
            }
        } else {
            cfg.sentry.accounts.push(SentryAccount {
                label: "sentry".into(),
                base_url: base,
                organizations: Vec::new(),
            });
        }
    }
    let _ = config::save(&cfg);
    let sentry_host = status
        .sentry_host
        .clone()
        .filter(|host| !host.is_empty())
        .or_else(host_from_config);
    web_session::set(WebSession {
        github: status.github,
        anthropic: status.anthropic,
        sentry: status.sentry,
        login: status.login.clone(),
        sentry_host,
    });
}

fn host_from_config() -> Option<String> {
    let base = config::load()
        .sentry
        .accounts
        .first()?
        .base_url
        .trim()
        .trim_end_matches('/')
        .to_string();
    let host = base
        .strip_prefix("https://")
        .or_else(|| base.strip_prefix("http://"))
        .unwrap_or(&base);
    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}

fn endpoint(path: &str) -> String {
    let origin = web_sys::window()
        .and_then(|window| window.location().origin().ok())
        .unwrap_or_default();
    format!("{origin}{path}")
}

async fn session_status() -> Result<Status, String> {
    let response = fastdash_core::engine::http::Client::new()
        .get(endpoint("/api/session"))
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if !response.status().is_success() {
        return Err(error_text(response).await);
    }
    let body: SessionJson = response.json().await.map_err(|err| err.to_string())?;
    if !body.signed_in.unwrap_or(false) && !body.github && !body.anthropic && !body.sentry {
        return Ok(Status {
            github: false,
            anthropic: false,
            sentry: false,
            login: None,
            sentry_host: None,
        });
    }
    Ok(Status {
        github: body.github,
        anthropic: body.anthropic,
        sentry: body.sentry,
        login: body.login.filter(|login| !login.is_empty()),
        sentry_host: host_from_config(),
    })
}

async fn create_session(
    github: String,
    anthropic: String,
    sentry: String,
    host: String,
) -> Result<Status, String> {
    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Body<'a> {
        #[serde(skip_serializing_if = "str::is_empty")]
        github_token: &'a str,
        #[serde(skip_serializing_if = "str::is_empty")]
        anthropic_admin_key: &'a str,
        #[serde(skip_serializing_if = "str::is_empty")]
        sentry_token: &'a str,
        #[serde(skip_serializing_if = "str::is_empty")]
        sentry_host: &'a str,
    }
    let response = fastdash_core::engine::http::Client::new()
        .post(endpoint("/api/session"))
        .json(&Body {
            github_token: &github,
            anthropic_admin_key: &anthropic,
            sentry_token: &sentry,
            sentry_host: &host,
        })
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if !response.status().is_success() {
        return Err(error_text(response).await);
    }
    let body: SessionJson = response.json().await.map_err(|err| err.to_string())?;
    Ok(Status {
        github: body.github,
        anthropic: body.anthropic,
        sentry: body.sentry,
        login: body.login.filter(|login| !login.is_empty()),
        sentry_host: if host.is_empty() { None } else { Some(host) },
    })
}

async fn delete_session() -> Result<(), String> {
    let response = fastdash_core::engine::http::Client::new()
        .delete(endpoint("/api/session"))
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(error_text(response).await)
    }
}

async fn error_text(response: fastdash_core::engine::http::Response) -> String {
    let code = response.status().as_u16();
    let text = response.text().await.unwrap_or_default();
    #[derive(Deserialize)]
    struct ErrBody {
        error: String,
    }
    serde_json::from_str::<ErrBody>(&text)
        .map(|body| body.error)
        .unwrap_or_else(|_| {
            if text.is_empty() {
                format!("HTTP {code}")
            } else {
                text
            }
        })
}

#[derive(Deserialize)]
struct SessionJson {
    #[serde(default, rename = "signedIn")]
    signed_in: Option<bool>,
    #[serde(default)]
    login: Option<String>,
    #[serde(default)]
    github: bool,
    #[serde(default)]
    anthropic: bool,
    #[serde(default)]
    sentry: bool,
}
