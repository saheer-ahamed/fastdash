// fastdash - a super-fast Claude usage + connectors dashboard.
//
// Architecture: a connector-agnostic `engine` (the Connector trait + generic
// render `Panel`s + a registry) with self-contained `connectors` plugged in
// behind that trait. The frontend only ever sees `Panel`s, so adding a
// connector requires zero UI changes.
//
// The engine also owns the cross-cutting infra: non-secret config
// (`engine::config`), the OS keychain wrapper (`engine::secrets`), the in-memory
// snapshot cache (`engine::cache`), and the background refresh scheduler
// (`engine::scheduler`). Shared state is wired in below via `.manage(...)` and
// the scheduler is spawned in `.setup(...)`.

// TODO: remove once every connector is fleshed out; keeps the scaffold quiet.
#![allow(dead_code)]

mod connectors;
mod engine;
mod ipc;

use std::sync::{Arc, RwLock};

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

    tauri::Builder::default()
        .manage(Arc::clone(&registry))
        .manage(Arc::clone(&cache))
        .manage(Arc::clone(&config))
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
        ])
        .setup(move |app| {
            engine::scheduler::start(app.handle().clone(), registry, cache, config);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running fastdash");
}
