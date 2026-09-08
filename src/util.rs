#[cfg(not(target_arch = "wasm32"))]
use std::sync::OnceLock;

/// Host-provided base-directory overrides (Kotlin bridge `initCore` sets these
/// to the app's `filesDir`/`cacheDir`: `dirs` has no Android mapping, so
/// without an override durable state would land in a temp directory).
#[cfg(not(target_arch = "wasm32"))]
static DATA_DIR_OVERRIDE: OnceLock<std::path::PathBuf> = OnceLock::new();
#[cfg(not(target_arch = "wasm32"))]
static CACHE_DIR_OVERRIDE: OnceLock<std::path::PathBuf> = OnceLock::new();

/// Pin the base directories for this process. First call wins; later calls are
/// ignored so the bridge init cannot reroute live paths.
#[cfg(not(target_arch = "wasm32"))]
pub fn set_dir_overrides(data_dir: std::path::PathBuf, cache_dir: std::path::PathBuf) {
    let _ = DATA_DIR_OVERRIDE.set(data_dir);
    let _ = CACHE_DIR_OVERRIDE.set(cache_dir);
}

/// Base directory for runtime-cached data (`~/Library/Caches`, `$XDG_CACHE_HOME`,
/// `%LOCALAPPDATA%`, …). Native-only: filesystem routing is replaced by
/// `crate::platform::storage` (localStorage) on wasm.
#[cfg(not(target_arch = "wasm32"))]
pub fn cache_dir() -> std::path::PathBuf {
    if let Some(dir) = CACHE_DIR_OVERRIDE.get() {
        return dir.clone();
    }
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("spotify-dx")
}

/// Base directory for durable local state (tokens, preferences). Native-only.
#[cfg(not(target_arch = "wasm32"))]
pub fn data_dir() -> std::path::PathBuf {
    if let Some(dir) = DATA_DIR_OVERRIDE.get() {
        return dir.clone();
    }
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("spotify-dx")
}