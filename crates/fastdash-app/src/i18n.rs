//! UI strings. One catalog is shared with fastdash-core (`locales/<lang>/*.json`),
//! so a string means the same thing on a panel and in the chrome around it.

use fastdash_core::engine::i18n;
use gpui::SharedString;

/// The string at a dotted key, e.g. `t("app.refresh")`.
pub fn t(key: &str) -> SharedString {
    i18n::t(key).into()
}

/// [`t`] with `{name}` placeholders filled from `args`.
pub fn tf(key: &str, args: &[(&str, &str)]) -> SharedString {
    i18n::tf(key, args).into()
}

/// Selectable languages. Add one: create `locales/<id>/*.json`, embed it in
/// fastdash-core's i18n, and list it here.
pub const LOCALES: &[(&str, &str)] = &[("en", "English")];
