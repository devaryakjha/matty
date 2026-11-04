//! The browser's `localStorage`, standing in for the files Matty keeps on
//! the desktop (settings) and the credential store (the session token).
//!
//! Keys are the paths the desktop would use, so the code above stays the
//! same. Everything here is per origin: `https://matty.jha.sh` sees only its
//! own entries.

use std::io;

fn storage() -> io::Result<web_sys::Storage> {
    web_sys::window()
        .and_then(|window| window.local_storage().ok().flatten())
        .ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported, "no localStorage"))
}

fn js_error(_: wasm_bindgen::JsValue) -> io::Error {
    io::Error::other("localStorage refused the change (full or disabled)")
}

/// The text kept under `key`.
pub fn get(key: &str) -> io::Result<String> {
    storage()?
        .get_item(key)
        .map_err(js_error)?
        .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
}

/// Keeps `value` under `key`, replacing what was there.
pub fn set(key: &str, value: &str) -> io::Result<()> {
    storage()?.set_item(key, value).map_err(js_error)
}

/// Forgets `key`.
pub fn remove(key: &str) -> io::Result<()> {
    storage()?.remove_item(key).map_err(js_error)
}
