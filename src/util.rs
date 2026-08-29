/// Base directory for runtime-cached data (`~/Library/Caches`, `$XDG_CACHE_HOME`,
/// `%LOCALAPPDATA%`, …). Native-only: filesystem routing is replaced by
/// `crate::platform::storage` (localStorage) on wasm.
#[cfg(not(target_arch = "wasm32"))]
pub fn cache_dir() -> std::path::PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("spotify-dx")
}

/// Base directory for durable local state (tokens, preferences). Native-only.
#[cfg(not(target_arch = "wasm32"))]
pub fn data_dir() -> std::path::PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("spotify-dx")
}