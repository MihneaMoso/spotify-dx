use base64::Engine as _;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// In-memory TTL cache (5 minutes), keyed by full request URL.
const MEMORY_TTL: Duration = Duration::from_secs(5 * 60);

struct MemoryEntry {
    bytes: Vec<u8>,
    created: Instant,
}

static MEMORY: Lazy<Mutex<HashMap<String, MemoryEntry>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Serve a request from memory when it is warm.
pub fn get(url: &str) -> Option<Vec<u8>> {
    let mut guard = MEMORY.lock();
    if let Some(entry) = guard.get(url) {
        if entry.created.elapsed() < MEMORY_TTL {
            return Some(entry.bytes.clone());
        }
        guard.remove(url);
    }
    None
}

/// Store a response body in the memory+TLS cache.
pub fn put(url: &str, bytes: Vec<u8>) {
    MEMORY.lock().insert(
        url.to_owned(),
        MemoryEntry {
            bytes: bytes.clone(),
            created: Instant::now(),
        },
    );
    persist(url, &bytes);
}

/// Where an endpoint's JSON lands on disk for offline reads.
fn cache_path_for(url: &str) -> PathBuf {
    let mut hasher = Sha256::new();
    hasher.update(url.as_bytes());
    let hash = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hasher.finalize());
    crate::util::cache_dir()
        .join("api")
        .join(format!("{hash}.json"))
}

fn persist(url: &str, bytes: &[u8]) {
    let path = cache_path_for(url);
    if let Some(dir) = path.parent() {
        if let Err(err) = std::fs::create_dir_all(dir) {
            tracing::debug!("cache: cannot create dir: {err}");
            return;
        }
    }
    if let Err(err) = std::fs::write(&path, bytes) {
        tracing::debug!("cache: cannot persist {url}: {err}");
    }
}

/// Best-effort read of the on-disk copy (offline / degraded mode).
pub fn get_from_disk(url: &str) -> Option<Vec<u8>> {
    std::fs::read(cache_path_for(url)).ok()
}

/// Purge everything (used by tests).
pub fn clear() {
    MEMORY.lock().clear();
}