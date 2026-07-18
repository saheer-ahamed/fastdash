// fastdash - a super-fast Claude usage + connectors dashboard.
//
// Architecture: a connector-agnostic `engine` (the Connector trait + generic
// render `Panel`s + a registry) with self-contained `connectors` plugged in
// behind that trait. The frontend only ever sees `Panel`s, so adding a
// connector requires zero UI changes.

// TODO: remove once every connector is fleshed out; keeps the scaffold quiet.
#![allow(dead_code)]

mod connectors;
mod engine;
mod ipc;

use engine::registry::Registry;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let registry = Registry::with_default_connectors();

    tauri::Builder::default()
        .manage(registry)
        .invoke_handler(tauri::generate_handler![
            ipc::list_connectors,
            ipc::fetch_connector,
        ])
        .run(tauri::generate_context!())
        .expect("error while running fastdash");
}
