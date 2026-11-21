//! Which credentials the browser session has. The tokens themselves stay in
//! the Worker. Connectors on wasm treat [`PLACEHOLDER`] as "ask the proxy"
//! and never send it.

use std::sync::Mutex;

/// Not a credential. The HTTP layer drops `Authorization` and `x-api-key`
/// before the request leaves the browser.
pub const PLACEHOLDER: &str = "worker-session";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WebSession {
    pub github: bool,
    pub anthropic: bool,
    pub sentry: bool,
    pub login: Option<String>,
    /// One extra Sentry hostname from the session, without a scheme.
    pub sentry_host: Option<String>,
}

static SESSION: Mutex<WebSession> = Mutex::new(WebSession {
    github: false,
    anthropic: false,
    sentry: false,
    login: None,
    sentry_host: None,
});

pub fn current() -> WebSession {
    SESSION
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

pub fn set(session: WebSession) {
    *SESSION
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = session;
}

pub fn clear() {
    set(WebSession::default());
}

pub fn has_github() -> bool {
    current().github
}

pub fn has_anthropic() -> bool {
    current().anthropic
}

pub fn has_sentry() -> bool {
    current().sentry
}
