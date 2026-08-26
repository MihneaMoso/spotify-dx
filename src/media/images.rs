//! Disk-cached artwork loader (Phase 4).
//! Keyed by SHA-256 of the image URL (`cache_dir/img_cache/`), with a 128-file
//! LRU cap. Every `load(url)` checks disk first; on miss, fetches via
//! `filtered_get` (through the ad-filter) then writes the snapshot back for
//! future warm starts.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use crate::app_error::AppError;
use crate::spotify::client;

/// How long a cached image snapshot stays fresh (long — artwork URLs are
/// stable for the lifetime of a release).
const IMAGE_CACHE_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60); // 30 days
/// Max image files kept on disk.
const IMAGE_CACHE_CAP: usize = 128;

/// Return the disk root for this module (`~/Library/Caches/spotify-dx/img_cache`).
fn root() -> PathBuf {
    crate::util::cache_dir().join("img_cache")
}

/// SHA-256 URL-safe keyed snapshot path.
fn path_for(url: &str) -> PathBuf {
    use base64::Engine as _;
    use sha2::Digest as _;
    let mut hasher = sha2::Sha256::new();
    hasher.update(url.as_bytes());
    let hash = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hasher.finalize());
    root().join(format!("{hash}.raw"))
}

/// Load image bytes for `url`: disk first (if fresh), else fetch through the
/// ad-filtered client, then write back to disk for future hits.
/// Returns an `Arc<Vec<u8>>` so multiple AlbumArt instances share the same
/// bytes (shared memory, not duplicated per render).
///
/// # Phase 4 — artwork caching layer
/// Before this module, every `AlbumArt` mount fetched the full image over the
/// network (even for warm restarts). After wiring this, the first load is
/// a cold miss; every subsequent visit hits the snapshot directly.
pub async fn load(url: &str) -> Result<std::sync::Arc<Vec<u8>>, AppError> {
    if url.is_empty() {
        return Err(AppError::Spotify("empty image url".into()));
    }
    let path = path_for(url);

    // Fast path: disk snapshot is fresh.
    if let Ok(meta) = std::fs::metadata(&path) {
        if let Ok(modified) = meta.modified() {
            let age = SystemTime::now().duration_since(modified).unwrap_or(Duration::MAX);
            if age < IMAGE_CACHE_TTL {
                if let Ok(bytes) = std::fs::read(&path) {
                    return Ok(std::sync::Arc::new(bytes));
                }
            }
        }
    }

    // Miss: fetch through the ad-filtered client.
    let resp = client::filtered_get(url).await?;
    let bytes = resp.error_for_status()?.bytes().await?.to_vec();

    // Write snapshot (best-effort; don't fail the image just because disk is full).
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, &bytes);
    trim_cache();

    Ok(std::sync::Arc::new(bytes))
}

/// Evict oldest snapshots if we're over the LRU cap. Simple approach: list by
/// file mtime, remove oldest until under cap.
fn trim_cache() {
    let dir = root();
    let Ok(mut entries) = std::fs::read_dir(&dir) else { return; };
    let mut files: Vec<(PathBuf, SystemTime)> = Vec::new();
    for entry in entries.flatten() {
        let p = entry.path();
        if p.extension().map(|e| e == "raw").unwrap_or(false) {
            if let Ok(meta) = entry.metadata() {
                if let Ok(modified) = meta.modified() {
                    files.push((p, modified));
                }
            }
        }
    }
    if files.len() > IMAGE_CACHE_CAP {
        files.sort_by_key(|(_, m)| *m);
        for (path, _) in files.iter().take(files.len() - IMAGE_CACHE_CAP) {
            let _ = std::fs::remove_file(path);
        }
    }
}
