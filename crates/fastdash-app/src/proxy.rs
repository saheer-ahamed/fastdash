//! Map a connector URL onto the Cloudflare Worker. The function lives in
//! `fastdash-core` so the connectors can call it; this module is the app-side
//! name documented in `docs/web.md`.
//!
//! The Worker (`cloudflare/src/index.js`) holds the token and adds the
//! `Authorization` / `x-api-key` header. Desktop builds do not rewrite URLs.

#[allow(unused_imports)]
pub use fastdash_core::engine::proxy::to_worker_path;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_app_reexports_the_core_mapper() {
        assert_eq!(
            to_worker_path("https://api.github.com/user").unwrap(),
            "/api/proxy/github/user"
        );
    }
}
