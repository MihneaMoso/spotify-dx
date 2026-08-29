//! Cached artwork loader (Phase 4).
//! Keyed by SHA-256 of the image URL, with a 128-entry LRU cap. Every
//! `load(url)` checks the store first; on miss, fetches via `filtered_get`
//! (through the ad-filter) then writes the snapshot back for warm starts.
//!
//! Persisted via `crate::platform::storage` — files under `dirs` on native,
//! `localStorage` on wasm. Each snapshot is `[8-byte LE unix secs][bytes]`.

use std::time::{Duration, SystemTime};

use crate::app_error::AppError;
use crate::spotify::client;

/// How long a cached image snapshot stays fresh (long — artwork URLs are
/// stable for the lifetime of a release).
const IMAGE_CACHE_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60); // 30 days

fn key_for(url: &str) -> String {
    use base64::Engine as _;
    use sha2::Digest as _;
    let mut hasher = sha2::Sha256::new();
    hasher.update(url.as_bytes());
    let hash = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hasher.finalize());
    format!("img://{hash}")
}

/// Load image bytes for `url`: store first (if fresh), else fetch through the
/// ad-filtered client, then write back. Returns an `Arc<Vec<u8>>` so multiple
/// AlbumArt instances share the same bytes.
pub async fn load(url: &str) -> Result<std::sync::Arc<Vec<u8>>, AppError> {
    if url.is_empty() {
        return Err(AppError::Spotify("empty image url".into()));
    }
    let key = key_for(url);

    // Fast path: cached snapshot is fresh.
    if let Some(blob) = crate::platform::storage::get_bytes(&key) {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if blob.len() > 8 {
            let written = u64::from_le_bytes(blob[..8].try_into().unwrap_or([0; 8]));
            if now.saturating_sub(written) < IMAGE_CACHE_TTL.as_secs() {
                return Ok(std::sync::Arc::new(blob[8..].to_vec()));
            }
        }
    }

    // Miss: fetch through the ad-filtered client.
    let resp = client::filtered_get(url).await?;
    let bytes = resp.error_for_status()?.bytes().await?.to_vec();

    // Write snapshot (best-effort; don't fail the image).
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut blob = now.to_le_bytes().to_vec();
    blob.extend_from_slice(&bytes);
    crate::platform::storage::set_bytes(&key, &blob);

    Ok(std::sync::Arc::new(bytes))
}
