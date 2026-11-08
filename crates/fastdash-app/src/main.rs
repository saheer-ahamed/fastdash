//! fastdash - a super-fast Claude usage + connectors dashboard, native on GPUI.
//!
//! Everything that is not UI lives in `fastdash-core`; this crate renders the
//! generic `Panel`s it produces and decides when to fetch. Nothing fetches on a
//! timer in the background: only the dashboard on screen, only while the window
//! has focus.

// A release build is a GUI app: no console window behind it.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
// The port lands as a stack of PRs, and shared helpers arrive before the views
// that use them. Removed once the GPUI app replaces the Tauri one.
#![allow(dead_code)]

mod app;
mod assets;
mod connectors;
mod dashboard;
mod fonts;
mod github_view;
mod i18n;
mod panels;
mod prefs;
mod range_filter;
mod runtime;
mod settings;
mod theme;
mod ui;

use std::sync::Arc;

use fastdash_core::engine::{config, i18n as core_i18n, registry::Registry};
use gpui::{prelude::*, px, size, App, Application, TitlebarOptions, WindowBounds, WindowOptions};

use app::Root;

fn main() {
    let config = config::load();
    core_i18n::set_locale(&config.locale);
    let registry = Arc::new(Registry::with_default_connectors());
    let ui_prefs = prefs::load();

    Application::new()
        .with_assets(assets::Assets)
        .run(move |cx: &mut App| {
            let choice = ui_prefs.theme;
            cx.set_global(ui_prefs);
            theme::apply(choice, cx.window_appearance(), cx);
            fonts::install(cx);
            ui::text_input::bind_keys(cx);
            ui::text_input::install_focus_actions(cx);

            let options = WindowOptions {
                window_bounds: Some(WindowBounds::centered(size(px(1120.), px(740.)), cx)),
                window_min_size: Some(size(px(880.), px(560.))),
                titlebar: Some(TitlebarOptions {
                    title: Some("fastdash".into()),
                    ..Default::default()
                }),
                app_id: Some("co.zro.fastdash".into()),
                ..Default::default()
            };
            cx.open_window(options, |window, cx| {
                cx.new(|cx| Root::new(registry, config, window, cx))
            })
            .expect("failed to open the main window");
            cx.activate(true);
        });
}
