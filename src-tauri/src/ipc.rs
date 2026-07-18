//! Tauri command surface the frontend calls.

use tauri::State;

use crate::engine::connector::{ConnectorMeta, FetchCtx, Snapshot};
use crate::engine::registry::Registry;

#[tauri::command]
pub fn list_connectors(registry: State<'_, Registry>) -> Vec<ConnectorMeta> {
    registry.all().iter().map(|c| c.meta()).collect()
}

#[tauri::command]
pub async fn fetch_connector(
    registry: State<'_, Registry>,
    id: String,
) -> Result<Snapshot, String> {
    let connector = registry
        .get(&id)
        .ok_or_else(|| format!("unknown connector: {id}"))?;

    // TODO: source timezone from AppConfig once config loading lands.
    let ctx = FetchCtx {
        timezone: "Asia/Kolkata".into(),
    };

    connector.fetch(&ctx).await.map_err(|e| e.to_string())
}
