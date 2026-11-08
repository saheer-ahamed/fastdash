//! The UI font: the first installed monospace face from the same stack the web
//! UI used (`ui-monospace, "Cascadia Mono", ..., monospace`).

use gpui::{App, Global, SharedString};

const STACK: &[&str] = &[
    "Cascadia Mono",
    "Cascadia Code",
    "SF Mono",
    "Menlo",
    "Consolas",
    "DejaVu Sans Mono",
    "Liberation Mono",
    "Courier New",
];

/// The resolved UI font family, a GPUI global.
#[derive(Clone)]
pub struct UiFont(pub SharedString);

impl Global for UiFont {}

/// Pick the first family in [`STACK`] that is installed. Asking the text
/// system for a missing family would silently fall back to a proportional
/// face, which breaks every column that relies on monospace alignment.
pub fn install(cx: &mut App) {
    let installed = cx.text_system().all_font_names();
    let family = STACK
        .iter()
        .find(|want| installed.iter().any(|have| have.eq_ignore_ascii_case(want)))
        .copied()
        .unwrap_or("monospace");
    cx.set_global(UiFont(family.into()));
}

pub fn family(cx: &App) -> SharedString {
    cx.global::<UiFont>().0.clone()
}
