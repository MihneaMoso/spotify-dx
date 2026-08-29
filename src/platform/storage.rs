//! Key/value storage seam.
//!
//! `set_bytes` / `get_bytes` / `remove` are the only storage primitive the app
//! needs. Native persists blobs under `cache_dir()` as files (auth tokens go
//! through the OS keychain instead — see `auth/token_store.rs`); wasm persists
//! to `localStorage` via `web-sys`. Keys must be filesystem/browser safe — the
//! native path sanitises them.

/// Persist a value under `key` (overwriting any existing value).
pub fn set_bytes(key: &str, bytes: &[u8]) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let path = path_for(key);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, bytes);
    }
    #[cfg(target_arch = "wasm32")]
    {
        let encoded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes);
        if let Some(storage) = window_storage() {
            let _ = storage.set_item(key, &encoded);
        }
    }
}

/// Load a value previously written with `set_bytes`. Returns `None` when absent.
pub fn get_bytes(key: &str) -> Option<Vec<u8>> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::fs::read(path_for(key)).ok()
    }
    #[cfg(target_arch = "wasm32")]
    {
        let storage = window_storage()?;
        let raw = storage.get_item(key).ok()??;
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, raw).ok()
    }
}

/// Delete a value previously written with `set_bytes`. Missing values are fine.
pub fn remove(key: &str) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = std::fs::remove_file(path_for(key));
    }
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(storage) = window_storage() {
            let _ = storage.remove_item(key);
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn path_for(key: &str) -> std::path::PathBuf {
    crate::util::data_dir().join(sanitize(key))
}

#[cfg(not(target_arch = "wasm32"))]
fn sanitize(key: &str) -> String {
    key.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(target_arch = "wasm32")]
fn window_storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}
