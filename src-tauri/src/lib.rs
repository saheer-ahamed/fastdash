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
        ])
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

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running fastdash");
}
