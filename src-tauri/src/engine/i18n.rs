//! Minimal i18n. Strings live in the shared catalog under `locales/<lang>/*.json`
//! (the same files the frontend imports), so there is one source of truth per
//! language. `t(key)` resolves a dotted key (e.g. "claude.tokensByModel") against
//! the catalog; `tf(key, args)` also substitutes `{name}` placeholders. A missing
//! key returns the key itself, so nothing renders blank.
//!
//! Only English is embedded today. Adding a language: drop `locales/<lang>/*.json`
//! and embed + select it here; every caller already goes through `t`/`tf`.

use std::sync::{OnceLock, RwLock};

use serde_json::{Map, Value};

// Selected locale, set once from config at startup. Stored for when more
// languages are embedded; today every lookup resolves against English.
static LOCALE: RwLock<String> = RwLock::new(String::new());

pub fn set_locale(lang: &str) {
    if let Ok(mut current) = LOCALE.write() {
        *current = lang.to_string();
    }
}

fn english() -> &'static Value {
    static CELL: OnceLock<Value> = OnceLock::new();
    CELL.get_or_init(|| {
        // One JSON file per area, merged at the top level.
        const FILES: &[&str] = &[
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../locales/en/app.json"
            )),
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../locales/en/claude.json"
            )),
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../locales/en/github.json"
            )),
        ];
        let mut root = Map::new();
        for raw in FILES {
            if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(raw) {
                for (k, v) in map {
                    root.insert(k, v);
                }
            }
        }
        Value::Object(root)
    })
}

/// Resolve a dotted key. Returns the key itself if it is missing.
pub fn t(key: &str) -> String {
    let mut node = english();
    for seg in key.split('.') {
        match node.get(seg) {
            Some(next) => node = next,
            None => return key.to_string(),
        }
    }
    node.as_str().unwrap_or(key).to_string()
}

/// `t` plus `{name}` placeholder substitution from `args`.
pub fn tf(key: &str, args: &[(&str, &str)]) -> String {
    let mut s = t(key);
    for (name, value) in args {
        s = s.replace(&format!("{{{name}}}"), value);
    }
    s
}
