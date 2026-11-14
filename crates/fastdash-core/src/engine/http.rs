//! HTTP for connectors.
//!
//! On the desktop this is `reqwest`, with the same TLS stack and the same
//! headers the connectors set. In the browser, GitHub, Anthropic, and Sentry
//! URLs are rewritten onto `/api/proxy/...` and `Authorization` / `x-api-key`
//! are removed. The session cookie is what the Worker trusts.

#[cfg(not(target_arch = "wasm32"))]
pub use reqwest::Client;

#[cfg(target_arch = "wasm32")]
mod browser {
    use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

    pub use reqwest::Response;

    use super::super::proxy::to_worker_path;
    use super::super::web_session;

    const SECRET_HEADERS: &[&str] = &["authorization", "x-api-key"];

    #[derive(Clone)]
    pub struct Client {
        inner: reqwest::Client,
    }

    impl Client {
        pub fn new() -> Self {
            Self::builder().build().expect("browser HTTP client")
        }

        pub fn builder() -> ClientBuilder {
            ClientBuilder {
                inner: reqwest::Client::builder(),
            }
        }

        pub fn get(&self, url: impl AsRef<str>) -> RequestBuilder {
            self.request(reqwest::Method::GET, url)
        }

        pub fn post(&self, url: impl AsRef<str>) -> RequestBuilder {
            self.request(reqwest::Method::POST, url)
        }

        pub fn delete(&self, url: impl AsRef<str>) -> RequestBuilder {
            self.request(reqwest::Method::DELETE, url)
        }

        fn request(&self, method: reqwest::Method, url: impl AsRef<str>) -> RequestBuilder {
            RequestBuilder {
                inner: self.inner.request(method, map_url(url.as_ref())),
            }
        }
    }

    pub struct ClientBuilder {
        inner: reqwest::ClientBuilder,
    }

    impl ClientBuilder {
        pub fn default_headers(mut self, headers: HeaderMap) -> Self {
            let mut kept = HeaderMap::new();
            for (name, value) in headers.iter() {
                if is_secret(name.as_str()) {
                    continue;
                }
                kept.append(name.clone(), value.clone());
            }
            self.inner = self.inner.default_headers(kept);
            self
        }

        pub fn user_agent(mut self, value: impl AsRef<str>) -> Self {
            self.inner = self.inner.user_agent(value.as_ref());
            self
        }

        pub fn build(self) -> reqwest::Result<Client> {
            Ok(Client {
                inner: self.inner.build()?,
            })
        }
    }

    pub struct RequestBuilder {
        inner: reqwest::RequestBuilder,
    }

    impl RequestBuilder {
        pub fn header<K, V>(mut self, key: K, value: V) -> Self
        where
            HeaderName: TryFrom<K>,
            HeaderValue: TryFrom<V>,
        {
            let Ok(name) = HeaderName::try_from(key) else {
                return self;
            };
            let Ok(value) = HeaderValue::try_from(value) else {
                return self;
            };
            if is_secret(name.as_str()) {
                return self;
            }
            let mut extra = HeaderMap::new();
            extra.insert(name, value);
            self.inner = self.inner.headers(extra);
            self
        }

        pub fn bearer_auth<T>(self, token: T) -> Self
        where
            T: std::fmt::Display,
        {
            let _ = token;
            self
        }

        pub fn query<T: serde::Serialize + ?Sized>(mut self, query: &T) -> Self {
            self.inner = self.inner.query(query);
            self
        }

        pub fn json<T: serde::Serialize + ?Sized>(mut self, json: &T) -> Self {
            self.inner = self.inner.json(json);
            self
        }

        pub fn form<T: serde::Serialize + ?Sized>(mut self, form: &T) -> Self {
            self.inner = self.inner.form(form);
            self
        }

        pub async fn send(self) -> reqwest::Result<reqwest::Response> {
            self.inner.fetch_credentials_include().send().await
        }
    }

    fn is_secret(name: &str) -> bool {
        SECRET_HEADERS
            .iter()
            .any(|header| header.eq_ignore_ascii_case(name))
    }

    /// Same-origin absolute URL when the host is one the Worker proxies.
    /// Anything else is left alone (device-flow hosts, for example) and fails
    /// closed in the browser instead of carrying a token.
    fn map_url(url: &str) -> String {
        if let Ok(path) = to_worker_path(url) {
            return absolute(path);
        }
        if let Some(path) = session_sentry(url) {
            return absolute(path);
        }
        url.to_string()
    }

    fn absolute(path: String) -> String {
        let origin = web_sys::window()
            .and_then(|window| window.location().origin().ok())
            .unwrap_or_default();
        format!("{origin}{path}")
    }

    /// The one extra Sentry hostname stored with the session, if this URL is it.
    fn session_sentry(url: &str) -> Option<String> {
        let host = web_session::current().sentry_host?;
        let rest = url.strip_prefix("https://")?;
        let (url_host, path) = match rest.split_once('/') {
            Some((host, path)) => (host, format!("/{path}")),
            None => (rest, "/".to_string()),
        };
        if !url_host.eq_ignore_ascii_case(&host) {
            return None;
        }
        Some(format!("/api/proxy/sentry/{url_host}{path}"))
    }
}

#[cfg(target_arch = "wasm32")]
pub use browser::{Client, Response};
