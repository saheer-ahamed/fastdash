//! fastdash - a super-fast Claude usage + connectors dashboard, native on GPUI.
//!
//! Everything that is not UI lives in `fastdash-core`; this crate renders the
//! generic `Panel`s it produces and decides when to fetch. Nothing fetches on a
//! timer: the visible tab is fetched when it has no data yet, and after that
//! only when the user hits Refresh.

// A release build is a GUI app: no console window behind it.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// One window, two platforms. Desktop uses the OS backends. wasm32 uses Zed's
// `gpui_web` platform (`gpui_platform::single_threaded_web`), and connector
// HTTP goes through `proxy` so tokens stay in the Worker.
mod proxy;

mod app;
mod assets;
mod connectors;
#[cfg(not(target_arch = "wasm32"))]
mod crash;
mod dashboard;
mod fonts;
mod github_view;
mod i18n;
mod panels;
mod platform;
mod prefs;
mod range_filter;
mod runtime;
mod settings;
mod theme;
#[cfg(all(not(target_arch = "wasm32"), any(windows, target_os = "macos")))]
mod tray;
mod ui;
#[cfg(not(target_arch = "wasm32"))]
mod updater;
mod welcome;

use std::sync::Arc;

use fastdash_core::engine::{config, i18n as core_i18n, registry::Registry};
#[cfg(not(target_arch = "wasm32"))]
use fastdash_core::taskbar;
#[cfg(not(target_arch = "wasm32"))]
use futures::channel::mpsc;
#[cfg(not(target_arch = "wasm32"))]
use futures::StreamExt;
use gpui::{
    prelude::*, px, size, App, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions,
};

use app::Root;

/// What the world outside the window - tray, taskbar readout, a second
/// launch - asks of the app. Delivered to one GPUI task that owns the window.
#[cfg(not(target_arch = "wasm32"))]
pub enum Command {
    Show,
    Quit,
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    let booted = web_time::Instant::now();
    fastdash_core::engine::perf::init();
    crash::install();
    let (commands, mut incoming) = mpsc::unbounded::<Command>();

    // A second launch (Start menu, shortcut, the installer's "run now") must
    // surface the window that is already open rather than start another copy
    // fighting over the same config file and keychain entries.
    let show = commands.clone();
    let Some(_instance) = platform::single_instance(move || {
        let _ = show.unbounded_send(Command::Show);
    }) else {
        return;
    };

    let config = config::load();
    core_i18n::set_locale(&config.locale);
    let registry = Arc::new(Registry::with_default_connectors());
    let ui_prefs = prefs::load();
    crash::set_dev_mode(ui_prefs.dev_mode);

    application()
        .with_assets(assets::Assets)
        .run(move |cx: &mut App| {
            let choice = ui_prefs.theme;
            cx.set_global(ui_prefs);
            cx.observe_global::<prefs::Prefs>(|cx| crash::set_dev_mode(prefs::get(cx).dev_mode))
                .detach();
            theme::apply(choice, cx.window_appearance(), cx);
            fonts::install(cx);
            ui::text_input::bind_keys(cx);
            ui::text_input::install_focus_actions(cx);

            let options = window_options(cx);
            let taskbar_settings = config.taskbar.clone();
            let window = cx
                .open_window(options, |window, cx| {
                    cx.new(|cx| {
                        let root = Root::new(registry, config, window, cx);
                        fastdash_core::engine::perf::record(
                            "app.startup",
                            booted.elapsed(),
                            0,
                            fastdash_core::engine::perf::CacheState::Miss,
                            true,
                        );
                        root
                    })
                })
                .expect("failed to open the main window");

            // Closing the dashboard hides it instead of quitting: the taskbar
            // readout and the tray keep the app one click away, and the
            // tray's Quit is the way out.
            let _ = window.update(cx, |_, window, cx| {
                window.on_window_should_close(cx, |window, cx| {
                    platform::hide(window, cx);
                    false
                });
            });

            #[cfg(any(windows, target_os = "macos"))]
            let tray = match tray::build(commands.clone()) {
                Ok(tray) => Some(tray),
                Err(err) => {
                    eprintln!("fastdash: no tray icon: {err}");
                    None
                }
            };

            // The readout: clicking it opens the dashboard; its hover-revealed
            // refresh icon re-reads the numbers, off the readout's own thread.
            let open = commands.clone();
            taskbar::start(
                move || {
                    let _ = open.unbounded_send(Command::Show);
                },
                move || {
                    let settings = config::load().taskbar;
                    runtime::detach(async move { taskbar::apply(&settings).await });
                },
            );
            runtime::detach(async move { taskbar::apply(&taskbar_settings).await });

            cx.spawn(async move |cx| {
                // Owned here so the icon lives exactly as long as the app.
                #[cfg(any(windows, target_os = "macos"))]
                let _tray = tray;
                while let Some(command) = incoming.next().await {
                    match command {
                        Command::Show => {
                            let _ = window.update(cx, |_, window, cx| platform::show(window, cx));
                        }
                        Command::Quit => {
                            cx.update(|cx| cx.quit());
                        }
                    }
                }
            })
            .detach();
            cx.activate(true);
        });
}

fn window_options(cx: &mut App) -> WindowOptions {
    let bounds = Bounds::centered(None, size(px(1120.), px(740.)), cx);
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(880.), px(560.))),
        titlebar: Some(TitlebarOptions {
            title: Some("fastdash".into()),
            ..Default::default()
        }),
        app_id: Some("co.zro.fastdash".into()),
        ..Default::default()
    }
}

/// Desktop: GPUI's platform crate for macOS and Linux. Windows is constructed
/// directly so GPUI's own manifest is not embedded beside ours.
fn application() -> Application {
    #[cfg(target_arch = "wasm32")]
    {
        gpui_platform::single_threaded_web()
    }
    #[cfg(target_os = "windows")]
    {
        let platform = std::rc::Rc::new(
            gpui_windows::WindowsPlatform::new(false)
                .expect("failed to initialize Windows platform"),
        );
        Application::with_platform(platform)
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        gpui_platform::application()
    }
}

/// The browser build. `main` is empty because `wasm-bindgen` starts [`start`].
#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn start() {
    gpui_platform::web_init();
    let handle = application()
        .with_assets(assets::Assets)
        .run_embedded(|cx: &mut App| open_browser(cx));
    // The browser event loop outlives this call. Dropping the handle drops
    // the window that was just opened.
    std::mem::forget(handle);
}

#[cfg(target_arch = "wasm32")]
fn open_browser(cx: &mut App) {
    let booted = web_time::Instant::now();
    fastdash_core::engine::perf::init();
    let config = config::load();
    core_i18n::set_locale(&config.locale);
    let registry = Arc::new(Registry::with_default_connectors());
    let ui_prefs = prefs::load();
    let choice = ui_prefs.theme;
    cx.set_global(ui_prefs);
    theme::apply(choice, cx.window_appearance(), cx);
    fonts::install(cx);
    ui::text_input::bind_keys(cx);
    ui::text_input::install_focus_actions(cx);

    let options = window_options(cx);
    cx.open_window(options, |window, cx| {
        cx.new(|cx| {
            let root = Root::new(registry, config, window, cx);
            fastdash_core::engine::perf::record(
                "app.startup",
                booted.elapsed(),
                0,
                fastdash_core::engine::perf::CacheState::Miss,
                true,
            );
            root
        })
    })
    .expect("failed to open the main window");
    cx.activate(true);
}
