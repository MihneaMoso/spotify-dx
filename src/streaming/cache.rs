//! Stream-URL cache: memory + disk, keyed by (track_id, provider).
//!
//! Stream URLs expire (TTL ~1 hour), so the cache is short-lived compared to
//! the API data store. On hit, the cached URL is returned immediately without
//! hitting the network.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Cache entry with an expiry timestamp.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedUrl {
    pub url: String,
    pub format: String,
    /// Unix epoch seconds when this URL expires.
    pub expires_at: u64,
}

impl CachedUrl {
    pub fn is_expired(&self) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        now >= self.expires_at
    }
}

/// Cache key: track ID + provider name.
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
struct CacheKey {
    track_id: String,
    provider: String,
}

/// Default TTL for stream URLs: 50 minutes (conservative, most expire at 1h).
const STREAM_URL_TTL: Duration = Duration::from_secs(50 * 60);

/// Maximum entries in the memory cache before LRU eviction.
const MEMORY_CAP: usize = 256;

struct CacheInner {
    memory: HashMap<CacheKey, CachedUrl>,
    /// Insertion order for FIFO eviction.
    order: Vec<CacheKey>,
}

static STREAM_CACHE: OnceLock<std::sync::Mutex<CacheInner>> = OnceLock::new();

fn inner() -> &'static std::sync::Mutex<CacheInner> {
    STREAM_CACHE.get_or_init(|| {
        std::sync::Mutex::new(CacheInner {
            memory: HashMap::new(),
            order: Vec::new(),
        })
    })
}

/// Look up a cached stream URL. Returns `None` on miss or expiry.
pub fn get(track_id: &str, provider: &str) -> Option<CachedUrl> {
    let key = CacheKey {
        track_id: track_id.to_string(),
        provider: provider.to_string(),
    };
    let guard = inner().lock().ok()?;
    let entry = guard.memory.get(&key)?;
    if entry.is_expired() {
        return None;
    }
    Some(entry.clone())
}

/// Store a resolved stream URL in the cache.
pub fn put(track_id: &str, provider: &str, url: &str, format: &str) {
    let key = CacheKey {
        track_id: track_id.to_string(),
        provider: provider.to_string(),
    };
    let expires_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() + STREAM_URL_TTL.as_secs())
        .unwrap_or(0);
    let entry = CachedUrl {
        url: url.to_string(),
        format: format.to_string(),
        expires_at,
    };
    if let Ok(mut guard) = inner().lock() {
        // FIFO eviction when at capacity.
        if guard.memory.len() >= MEMORY_CAP {
            if let Some(oldest) = guard.order.first().cloned() {
                guard.memory.remove(&oldest);
                guard.order.remove(0);
            }
        }
        guard.memory.insert(key.clone(), entry);
        guard.order.push(key);
    }
}

/// Disk-persisted cache for surviving restarts.
fn disk_path() -> PathBuf {
    crate::util::cache_dir().join("stream_url_cache.json")
}

/// Save the memory cache to disk (best-effort, called on shutdown).
pub fn save_to_disk() {
    let snapshot: Vec<(CacheKey, CachedUrl)> = {
        let Ok(guard) = inner().lock() else { return };
        guard
            .memory
            .iter()
            .filter(|(_, v)| !v.is_expired())
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    };
    let path = disk_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(
        &path,
        serde_json::to_string_pretty(&snapshot).unwrap_or_default(),
    );
}

/// Load the disk cache into memory on startup (best-effort).
pub fn load_from_disk() {
    let path = disk_path();
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return;
    };
    let Ok(entries): Result<Vec<(CacheKey, CachedUrl)>, _> = serde_json::from_str(&raw) else {
        return;
    };
    if let Ok(mut guard) = inner().lock() {
        for (key, entry) in entries {
            if !entry.is_expired() {
                guard.memory.insert(key.clone(), entry);
                guard.order.push(key);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_get_roundtrip() {
        put("track1", "tidal", "https://example.com/stream.flac", "flac");
        let cached = get("track1", "tidal");
        assert!(cached.is_some());
        let c = cached.unwrap();
        assert_eq!(c.url, "https://example.com/stream.flac");
        assert_eq!(c.format, "flac");
    }

    #[test]
    fn miss_returns_none() {
        assert!(get("nonexistent", "tidal").is_none());
    }

    #[test]
    fn different_providers_are_separate() {
        put("t1", "tidal", "https://tidal.example.com", "flac");
        put("t1", "qobuz", "https://qobuz.example.com", "flac");
        assert!(get("t1", "tidal").is_some());
        assert!(get("t1", "qobuz").is_some());
        assert!(get("t1", "youtube").is_none());
    }

    #[test]
    fn expired_entry_returns_none() {
        let key = CacheKey {
            track_id: "expired_track".to_string(),
            provider: "test".to_string(),
        };
        let entry = CachedUrl {
            url: "https://expired.example.com".to_string(),
            format: "mp3".to_string(),
            expires_at: 0, // already expired
        };
        if let Ok(mut guard) = inner().lock() {
            guard.memory.insert(key.clone(), entry);
            guard.order.push(key);
        }
        assert!(get("expired_track", "test").is_none());
    }
}
