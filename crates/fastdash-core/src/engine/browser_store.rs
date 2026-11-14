//! `localStorage` for the browser build's non-secret config. Tokens never
//! go here.

#[cfg(target_arch = "wasm32")]
pub fn get(key: &str) -> Option<String> {
    let storage = web_sys::window()?.local_storage().ok()??;
    storage.get_item(key).ok()?
}

#[cfg(target_arch = "wasm32")]
pub fn set(key: &str, value: &str) -> Result<(), String> {
    let storage = web_sys::window()
        .and_then(|window| window.local_storage().ok().flatten())
        .ok_or_else(|| "this browser has no local storage".to_string())?;
    storage
        .set_item(key, value)
        .map_err(|_| "could not write local storage".to_string())
}

#[cfg(not(target_arch = "wasm32"))]
pub fn get(_key: &str) -> Option<String> {
    None
}

#[cfg(not(target_arch = "wasm32"))]
pub fn set(_key: &str, _value: &str) -> Result<(), String> {
    Ok(())
}
