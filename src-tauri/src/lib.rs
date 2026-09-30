// fastdash - a super-fast Claude usage + connectors dashboard.
//
// Architecture: a connector-agnostic `engine` (the Connector trait + generic
// render `Panel`s + a registry) with self-contained `connectors` plugged in
// behind that trait. The frontend only ever sees `Panel`s, so adding a
// connector requires zero UI changes.
//
// The engine also owns the cross-cutting infra: non-secret config
// (`engine::config`), the OS keychain wrapper (`engine::secrets`), the in-memory
// snapshot cache (`engine::cache`), and the shared fetch path
// (`engine::refresh`). Shared state is wired in below via `.manage(...)`.
//
// Nothing fetches on a timer here. The frontend drives every fetch and only for
// the dashboard on screen, only while the window has focus, so a backgrounded
// app makes no network calls at all.

// TODO: remove once every connector is fleshed out; keeps the scaffold quiet.
#![allow(dead_code)]

mod connectors;
mod engine;
mod ipc;
mod taskbar;

use std::sync::{Arc, RwLock};

use tauri::Manager;

use engine::cache::SnapshotCache;
use engine::config::{self, AppConfig};
use engine::registry::Registry;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let registry = Arc::new(Registry::with_default_connectors());
    let cache = Arc::new(SnapshotCache::new());
    let loaded = config::load();
    engine::i18n::set_locale(&loaded.locale);
    let config: Arc<RwLock<AppConfig>> = Arc::new(RwLock::new(loaded));

    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default();

    // In-app auto-update (desktop only): the frontend calls the updater plugin on
    // launch, and the process plugin relaunches once the signed installer runs.
    #[cfg(desktop)]
    {
        builder = builder
            // Registered first, as the plugin requires: a second launch has to be
            // short-circuited before the rest of the app spins up. The callback
            // runs in the *already running* instance, so all it does is surface
            // the existing window - restore it if minimised, raise it, focus it.
            .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
                show_main(app);
            }))
            .plugin(tauri_plugin_updater::Builder::new().build())
            .plugin(tauri_plugin_process::init());
    }

    builder
        .manage(registry)
        .manage(cache)
        .manage(config)
        .invoke_handler(tauri::generate_handler![
            ipc::list_connectors,
            ipc::fetch_connector,
            ipc::get_cached,
            ipc::get_config,
            ipc::save_config,
            ipc::set_secret,
            ipc::has_secret,
            ipc::delete_secret,
            ipc::github_device_start,
            ipc::github_device_poll,
            ipc::github_fetch,
            ipc::claude_connect,
            ipc::claude_disconnect,
            ipc::open_external,
            ipc::taskbar_refresh,
        ])
        .on_window_event(|window, event| {
            // Closing the dashboard hides it instead of quitting: the taskbar
            // readout and the tray icon keep the app one click away, and
            // "Quit" in the tray menu is the way out.
            #[cfg(desktop)]
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .setup(|app| {
            // Force the window icon at runtime so the taskbar picks it up. In
            // `tauri dev` on Windows the default icon path is not reliably
            // applied to the taskbar; explicitly calling `set_icon` sends
            // WM_SETICON and makes the taskbar/titlebar icon show up in dev too.
            #[cfg(desktop)]
            if let (Some(window), Some(icon)) = (
                app.get_webview_window("main"),
                app.default_window_icon().cloned(),
            ) {
                let _ = window.set_icon(icon);
            }

            #[cfg(desktop)]
            {
                tray::build(app.handle())?;
                let handle = app.handle().clone();
                taskbar::start(move || show_main(&handle));
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running fastdash");
}

/// Bring the dashboard back: from the tray, the taskbar readout, or a second
/// launch.
#[cfg(desktop)]
fn show_main(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// The notification-area icon: the way back to a hidden dashboard when the
/// taskbar readout is off, and the only way to quit now that closing the
/// window only hides it.
#[cfg(desktop)]
mod tray {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    use tauri::AppHandle;

    use crate::engine::i18n;

    pub fn build(app: &AppHandle) -> tauri::Result<()> {
        let open = MenuItem::with_id(app, "open", i18n::t("tray.open"), true, None::<&str>)?;
        let quit = MenuItem::with_id(app, "quit", i18n::t("tray.quit"), true, None::<&str>)?;
        let menu = Menu::with_items(app, &[&open, &quit])?;

        let mut tray = TrayIconBuilder::with_id("main")
            .tooltip("fastdash")
            .menu(&menu)
            .show_menu_on_left_click(false)
            .on_menu_event(|app, event| match event.id.as_ref() {
                "open" => super::show_main(app),
                "quit" => app.exit(0),
                _ => {}
            })
            .on_tray_icon_event(|tray, event| {
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
                {
                    super::show_main(tray.app_handle());
                }
            });
        if let Some(icon) = app.default_window_icon().cloned() {
            tray = tray.icon(icon);
        }
        tray.build(app)?;
        Ok(())
    }
}
