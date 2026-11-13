//! Map a connector URL onto the Cloudflare Worker, so the browser copy of this
//! app never puts a GitHub, Anthropic, or Sentry token on the request.
//!
//! The Worker (`cloudflare/src/index.js`) holds the token and adds the
//! `Authorization` / `x-api-key` header. This module is the other half of that
//! contract. It is not a second UI: the GPUI views stay where they are, and
//! they cannot be linked for wasm32 until GPUI grows a browser platform.
//! See `docs/web.md`.

/// Turn an upstream https URL into a same-origin Worker path.
///
/// Unknown hosts are rejected here so a bug cannot ask the Worker to fetch
/// an arbitrary server. Self-hosted Sentry is not in this list; the desktop
/// app still talks to it directly with the keychain token.
pub fn to_worker_path(upstream: &str) -> Result<String, &'static str> {
    let rest = upstream
        .strip_prefix("https://")
        .ok_or("only https URLs are proxied")?;
    if rest.contains('@') {
        return Err("URLs with credentials are not proxied");
    }
    let (host, path_and_query) = match rest.split_once('/') {
        Some((host, path)) => (host, format!("/{path}")),
        None => (rest, "/".to_string()),
    };
    if host.is_empty() || host.contains(':') {
        return Err("that host is not proxied");
    }
    let prefix = match host {
        "api.github.com" => "/api/proxy/github",
        "api.anthropic.com" => "/api/proxy/anthropic",
        host if is_sentry_saas(host) => {
            return Ok(format!("/api/proxy/sentry/{host}{path_and_query}"));
        }
        _ => return Err("that host is not proxied"),
    };
    Ok(format!("{prefix}{path_and_query}"))
}

fn is_sentry_saas(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    if host == "sentry.io" {
        return true;
    }
    let Some(prefix) = host.strip_suffix(".sentry.io") else {
        return false;
    };
    !prefix.is_empty()
        && prefix
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'.')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_and_anthropic_keep_their_path_and_query() {
        assert_eq!(
            to_worker_path("https://api.github.com/search/issues?q=type:pr").unwrap(),
            "/api/proxy/github/search/issues?q=type:pr"
        );
        assert_eq!(
            to_worker_path("https://api.github.com/graphql").unwrap(),
            "/api/proxy/github/graphql"
        );
        assert_eq!(
            to_worker_path("https://api.anthropic.com/v1/organizations/me").unwrap(),
            "/api/proxy/anthropic/v1/organizations/me"
        );
    }

    #[test]
    fn sentry_regions_stay_on_the_host() {
        assert_eq!(
            to_worker_path("https://de.sentry.io/api/0/organizations/").unwrap(),
            "/api/proxy/sentry/de.sentry.io/api/0/organizations/"
        );
        assert_eq!(
            to_worker_path("https://sentry.io/api/0/projects/").unwrap(),
            "/api/proxy/sentry/sentry.io/api/0/projects/"
        );
    }

    #[test]
    fn anything_else_is_refused() {
        assert!(to_worker_path("http://api.github.com/user").is_err());
        assert!(to_worker_path("https://user:token@api.github.com/user").is_err());
        assert!(to_worker_path("https://example.com/").is_err());
        assert!(to_worker_path("https://evil.sentry.io.attacker.com/").is_err());
        assert!(to_worker_path("https://sentry.example.com/api/0/").is_err());
    }
}
