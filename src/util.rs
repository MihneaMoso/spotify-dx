use std::path::PathBuf;

/// Base directory for runtime-cached data (`~/Library/Caches`, `$XDG_CACHE_HOME`,
/// `%LOCALAPPDATA%`, …).
pub fn cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("spotify-dx")
}

/// Base directory for durable local state (tokens, preferences).
pub fn data_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("spotify-dx")
}

/// Locate a file bundled under `assets/` at runtime. The bundled asset is
/// compiled in at build time, so this is a fallback for tools that re-serve the
/// asset directory wholesale.
pub fn bundled_asset(name: &str) -> Option<PathBuf> {
    let candidate = std::env::current_dir()
        .ok()?
        .join("assets")
        .join(name);
    candidate.is_file().then_some(candidate)
}