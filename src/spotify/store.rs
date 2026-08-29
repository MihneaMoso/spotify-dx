//! Request store: the performance tier every API GET flows through
//! (`SYSTEM_DESIGN.md` §6.1).
//!
//! Layers, innermost first:
//!
//! 1. **In-flight coalescing** (single-flight): N concurrent callers for the
//!    same key share ONE fetch. Later arrivals subscribe to a
//!    `tokio::sync::watch` channel fed by the leader. Prefetching becomes
//!    free: an in-flight warm-up and a page mount JOIN instead of duplicating.
//! 2. **Memory TTL cache** (5 min, FIFO-capped): instant session hits.
//! 3. **Disk snapshots, stale-while-revalidate**: when memory is cold, a
//!    snapshot younger than [`SWR_WINDOW`] returns IMMEDIATELY while a
//!    background refresh re-runs the loader. Cold starts and rate-limit
//!    windows paint UI from yesterday's snapshot instead of spinning.
//!
//! Errors are never cached. Followers observe failures through a cheap
//! Clone-able [`Fail`] summary (the leader keeps the rich original).
//! Instances own their storage root so tests run fully isolated; production
//! mounts [`Store::global()`].

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use parking_lot::Mutex;
use tokio::sync::watch;

use crate::app_error::AppError;

/// How long a memory-cached body stays fresh.
pub const MEMORY_TTL: Duration = Duration::from_secs(5 * 60);
/// Max bodies held in RAM (oldest-inserted evicted first).
pub const MEMORY_CAP: usize = 256;
/// Disk snapshots older than this are ignored entirely.
pub const SWR_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);

/// Clone-able failure summary handed to coalesced followers.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Fail {
    RateLimited,
    Auth,
    AdBlock,
    Generic(String),
}

impl From<&AppError> for Fail {
    fn from(err: &AppError) -> Self {
        match err {
            AppError::RateLimited => Fail::RateLimited,
            AppError::Auth(_) | AppError::SessionExpired => Fail::Auth,
            AppError::AdBlock(_) => Fail::AdBlock,
            other => Fail::Generic(other.to_string()),
        }
    }
}

impl Fail {
    fn into_app_error(self) -> AppError {
        match self {
            Fail::RateLimited => AppError::RateLimited,
            Fail::Auth => AppError::Auth("session revoked during coalesced fetch".into()),
            Fail::AdBlock => AppError::AdBlock("coalesced request".into()),
            Fail::Generic(msg) => AppError::Spotify(msg),
        }
    }
}

/// What subscribers receive: body bytes or a failure summary.
#[derive(Debug, Clone)]
enum Slot {
    Pending,
    Ok(Vec<u8>),
    Failed(Fail),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SlotResolution {
    Ok(Vec<u8>),
    Failed(Fail),
    Dropped,
}

impl Slot {
    fn resolution(&self) -> SlotResolution {
        match self {
            Slot::Pending => SlotResolution::Dropped,
            Slot::Ok(bytes) => SlotResolution::Ok(bytes.clone()),
            Slot::Failed(fail) => SlotResolution::Failed(fail.clone()),
        }
    }
}

impl Slot {
    fn is_pending(&self) -> bool {
        matches!(self, Slot::Pending)
    }
}

struct MemoryEntry {
    bytes: Vec<u8>,
    created: Instant,
}

struct Inner {
    memory: Mutex<HashMap<String, MemoryEntry>>,
    order: Mutex<VecDeque<String>>,
    inflight: Mutex<HashMap<String, watch::Sender<Slot>>>,
}

/// Cheap-clonable handle; all state lives behind one `Arc`.
#[derive(Clone)]
pub struct Store {
    // Filesystem root for disk snapshots (native). Deliberately unused on wasm,
    // where `disk_get_fresh`/`disk_put` route through `platform::storage`.
    #[allow(dead_code)]
    root: PathBuf,
    inner: Arc<Inner>,
}

impl Store {
    /// Create a store whose disk snapshots land under `root/api`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            inner: Arc::new(Inner {
                memory: Mutex::new(HashMap::new()),
                order: Mutex::new(VecDeque::new()),
                inflight: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// Production instance rooted at the app cache directory (native) or the
    /// browser storage seam (wasm).
    pub fn global() -> &'static Store {
        use once_cell::sync::Lazy;
        #[cfg(not(target_arch = "wasm32"))]
        {
            static GLOBAL: Lazy<Store> = Lazy::new(|| Store::new(crate::util::cache_dir()));
            &GLOBAL
        }
        #[cfg(target_arch = "wasm32")]
        {
            static GLOBAL: Lazy<Store> = Lazy::new(|| Store::new(std::path::PathBuf::new()));
            &GLOBAL
        }
    }

    /// SHA-256 URL-safe snapshot key (storage key on wasm, path basename native).
    fn snap_key(&self, key: &str) -> String {
        use base64::Engine as _;
        use sha2::Digest as _;
        let mut hasher = sha2::Sha256::new();
        hasher.update(key.as_bytes());
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hasher.finalize())
    }

    /// SHA-256 URL-safe keyed snapshot path (deterministic — tested). Native only.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn path_for(&self, key: &str) -> PathBuf {
        self.root.join("api").join(format!("{}.json", self.snap_key(key)))
    }

    fn inflight_has(&self, key: &str) -> bool {
        self.inner.inflight.lock().contains_key(key)
    }

    /// Resolve `key`: memory hit → join in-flight → disk SWR (stale now +
    /// background refresh) → lead the fetch. `allow_stale` gates layer 3.
    ///
    /// The loader runs AT MOST once per key per instant (single-flight); it is
    /// consumed either inline by the leader or moved into the background
    /// refresh task on a stale hit — never both.
    pub async fn resolve<Fut>(
        self,
        key: String,
        allow_stale: bool,
        load: impl FnOnce(String) -> Fut + Send + 'static,
    ) -> Result<Vec<u8>, AppError>
    where
        Fut: Future<Output = Result<Vec<u8>, AppError>> + Send + 'static,
    {
        // Layer 2: memory TTL.
        if let Some(hit) = self.memory_get(&key) {
            return Ok(hit);
        }

        // Layer 1 (follower side): join an existing fetch.
        let joined_rx = { self.inner.inflight.lock().get(&key).map(|e| e.subscribe()) };
        if let Some(mut rx) = joined_rx {
            return match wait_for_slot(&mut rx).await {
                SlotResolution::Ok(bytes) => Ok(bytes),
                SlotResolution::Failed(fail) => Err(fail.into_app_error()),
                SlotResolution::Dropped => match self.memory_get(&key) {
                    Some(hit) => Ok(hit),
                    None => Err(AppError::Spotify("coalesced request dropped".into())),
                },
            };
        }

        // Layer 3: disk stale-while-revalidate.
        if allow_stale && !self.inflight_has(&key) {
            if let Some(stale) = self.disk_get_fresh(&key, SWR_WINDOW) {
                tracing::debug!("store: STALE disk snapshot serves {key}");
                tokio::spawn(async move {
                    let _ = self.leader(&key, load).await;
                });
                return Ok(stale);
            }
        }

        self.leader(&key, load).await
    }


    /// Leader path: claim the inflight slot under one lock (loser becomes a
    /// follower on the same sender), run the loader inline, publish caches,
    /// then wake followers.
    async fn leader<Fut>(
        &self,
        key: &str,
        load: impl FnOnce(String) -> Fut,
    ) -> Result<Vec<u8>, AppError>
    where
        Fut: Future<Output = Result<Vec<u8>, AppError>> + Send + 'static,
    {
        let (we_lead, tx) = {
            let mut guard = self.inner.inflight.lock();
            match guard.get(key) {
                Some(existing) => (false, existing.clone()),
                None => {
                    let (tx, _) = watch::channel(Slot::Pending);
                    guard.insert(key.to_owned(), tx.clone());
                    (true, tx)
                }
            }
        };

        if !we_lead {
            let mut rx = tx.subscribe();
            return match wait_for_slot(&mut rx).await {
                SlotResolution::Ok(bytes) => Ok(bytes),
                SlotResolution::Failed(fail) => Err(fail.into_app_error()),
                SlotResolution::Dropped => match self.memory_get(key) {
                    Some(hit) => Ok(hit),
                    None => Err(AppError::Spotify("coalesced request dropped".into())),
                },
            };
        }

        let res = load(key.to_owned()).await;

        // Publish: caches BEFORE removing the inflight entry so late joiners
        // either catch the watch send or hit memory — never re-fetch.
        if let Ok(bytes) = &res {
            self.memory_put(key, bytes);
            self.disk_put(key, bytes);
        }
        self.inner.inflight.lock().remove(key);
        let _ = tx.send(match &res {
            Ok(bytes) => Slot::Ok(bytes.clone()),
            Err(err) => Slot::Failed(Fail::from(err)),
        });

        res
    }

    fn memory_get(&self, key: &str) -> Option<Vec<u8>> {
        let mut guard = self.inner.memory.lock();
        if let Some(entry) = guard.get(key) {
            if entry.created.elapsed() < MEMORY_TTL {
                return Some(entry.bytes.clone());
            }
            guard.remove(key);
        }
        None
    }

    fn memory_put(&self, key: &str, bytes: &[u8]) {
        self.inner.memory.lock().insert(
            key.to_owned(),
            MemoryEntry {
                bytes: bytes.to_vec(),
                created: Instant::now(),
            },
        );
        let mut order = self.inner.order.lock();
        order.push_back(key.to_owned());
        while order.len() > MEMORY_CAP {
            if let Some(oldest) = order.pop_front() {
                self.inner.memory.lock().remove(&oldest);
            }
        }
    }

    /// Disk snapshot younger than `max_age`, judged by file mtime (native) or a
    /// timestamp header in the stored blob (wasm).
    pub fn disk_get_fresh(&self, key: &str, max_age: Duration) -> Option<Vec<u8>> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let path = self.path_for(key);
            let meta = std::fs::metadata(&path).ok()?;
            let modified = meta.modified().ok()?;
            let age = SystemTime::now().duration_since(modified).ok()?;
            if age >= max_age {
                return None;
            }
            std::fs::read(path).ok()
        }
        #[cfg(target_arch = "wasm32")]
        {
            let blob = crate::platform::storage::get_bytes(&format!("api://{}", self.snap_key(key)))?;
            let now = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            if blob.len() > 8 {
                let written = u64::from_le_bytes(blob[..8].try_into().unwrap_or([0; 8]));
                if now.saturating_sub(written) < max_age.as_secs() {
                    return Some(blob[8..].to_vec());
                }
            }
            None
        }
    }

    pub fn disk_put(&self, key: &str, bytes: &[u8]) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let path = self.path_for(key);
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(path, bytes);
        }
        #[cfg(target_arch = "wasm32")]
        {
            let now = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let mut blob = now.to_le_bytes().to_vec();
            blob.extend_from_slice(bytes);
            crate::platform::storage::set_bytes(&format!("api://{}", self.snap_key(key)), &blob);
        }
    }
}

/// Block until the channel observes a non-Pending slot. A dropped sender
/// (leader panicked) maps to `Dropped`; the caller falls back to memory.
async fn wait_for_slot(rx: &mut watch::Receiver<Slot>) -> SlotResolution {
    // Fast path: already a value beyond Pending.
    if !rx.borrow().is_pending() {
        return rx.borrow().resolution();
    }
    loop {
        match rx.changed().await {
            Ok(()) => {
                let slot = rx.borrow().clone();
                if !slot.is_pending() {
                    return slot.resolution();
                }
            }
            Err(_) => return SlotResolution::Dropped,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("spotify-dx-store-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[tokio::test]
    async fn coalesces_concurrent_callers_into_one_load() {
        let store = Store::new(temp_root("coalesce"));
        let calls = Arc::new(AtomicUsize::new(0));

        let ls = store.clone();
        let lc = calls.clone();
        let leader = tokio::spawn(async move {
            ls.resolve("k".into(), false, |k| async move {
                lc.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok::<Vec<u8>, AppError>(format!("body-{k}").into_bytes())
            })
            .await
        });
        tokio::time::sleep(Duration::from_millis(10)).await;

        let mut followers = Vec::new();
        for _ in 0..5 {
            let s = store.clone();
            followers.push(tokio::spawn(async move {
                s.resolve("k".into(), false, |k| async move {
                    Ok::<Vec<u8>, AppError>(format!("dup-{k}").into_bytes())
                })
                .await
            }));
        }

        assert_eq!(leader.await.unwrap().unwrap(), b"body-k".to_vec());
        for f in followers {
            assert_eq!(f.await.unwrap().unwrap(), b"body-k".to_vec());
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1); // ONE load despite six callers
    }

    #[tokio::test]
    async fn memory_ttl_serves_repeat_without_loading() {
        let store = Store::new(temp_root("ttl"));
        let calls = Arc::new(AtomicUsize::new(0));
        let key = "ttl-key".to_string();

        let c1 = calls.clone();
        let r1 = store
            .clone()
            .resolve(key.clone(), false, move |k| {
                let c = c1.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Ok::<Vec<u8>, AppError>(format!("v1-{k}").into_bytes())
                }
            })
            .await
            .unwrap();
        // Pure memory hit: the second loader must never run.
        let c2 = calls.clone();
        let r2 = store.clone().resolve(key, false, move |k| {
            let c = c2.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok::<Vec<u8>, AppError>(format!("v2-{k}").into_bytes())
            }
        }).await.unwrap();
        assert_eq!(r1, r2);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn errors_are_never_cached() {
        let store = Store::new(temp_root("errors"));
        let attempt = Arc::new(AtomicUsize::new(0));
        let a = attempt.clone();
        let r = store.clone().resolve("e".into(), false, move |_| {
            let a = a.clone();
            async move {
                if a.fetch_add(1, Ordering::SeqCst) == 0 {
                    Err(AppError::RateLimited)
                } else {
                    Ok::<Vec<u8>, AppError>(b"recovered".to_vec())
                }
            }
        }).await;
        assert!(matches!(r, Err(AppError::RateLimited)));
        let r2 = store
            .clone()
            .resolve("e".into(), false, |_| async {
                Ok::<Vec<u8>, AppError>(b"recovered".to_vec())
            })
            .await
            .unwrap();
        assert_eq!(r2, b"recovered".to_vec());
    }

    #[tokio::test]
    async fn follower_of_failed_fetch_receives_typed_error() {
        let store = Store::new(temp_root("fail"));
        let calls = Arc::new(AtomicUsize::new(0));
        let ls = store.clone();
        let lc = calls.clone();
        let leader = tokio::spawn(async move {
            ls.resolve("f".into(), false, |k| async move {
                lc.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(30)).await;
                let _ = k;
                Err::<Vec<u8>, AppError>(AppError::RateLimited)
            })
            .await
        });
        tokio::time::sleep(Duration::from_millis(5)).await;

        let s2 = store.clone();
        let joiner = tokio::spawn(async move {
            s2.resolve("f".into(), false, |k| async move {
                Ok::<Vec<u8>, AppError>(format!("{k}").into_bytes())
            })
            .await
        });

        assert!(matches!(leader.await.unwrap(), Err(AppError::RateLimited)));
        assert!(matches!(joiner.await.unwrap(), Err(AppError::RateLimited)));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn disk_roundtrip_and_staleness_window() {
        let root = temp_root("disk");
        let store = Store::new(&root);
        let key = "snap";
        store.disk_put(key, b"snapshot-bytes");
        assert!(store.path_for(key).exists());

        assert_eq!(
            store.disk_get_fresh(key, SWR_WINDOW).as_deref(),
            Some(b"snapshot-bytes".as_ref())
        );
        assert_eq!(store.disk_get_fresh(key, Duration::from_nanos(1)), None);
        assert_eq!(store.path_for(key), Store::new(&root).path_for(key));
    }

    #[tokio::test]
    async fn stale_hit_returns_immediately_then_memory_serves_fresh() {
        let store = Store::new(temp_root("swr"));
        store.disk_put("warm", b"STALE-BYTES");

        let t0 = Instant::now();
        let res = store
            .clone()
            .resolve("warm".into(), true, |k| async move {
                tokio::time::sleep(Duration::from_millis(80)).await;
                Ok::<Vec<u8>, AppError>(format!("FRESH-{k}").into_bytes())
            })
            .await
            .unwrap();
        assert_eq!(res, b"STALE-BYTES");
        assert!(t0.elapsed() < Duration::from_millis(40), "SWR must not block");

        tokio::time::sleep(Duration::from_millis(200)).await;
        let fresh = store
            .clone()
            .resolve("warm".into(), false, |k| async move {
                Ok::<Vec<u8>, AppError>(format!("NEVER-{k}").into_bytes())
            })
            .await
            .unwrap();
        assert_eq!(fresh, b"FRESH-warm");
    }

    #[tokio::test]
    async fn fifo_cap_evicts_oldest_entries() {
        let store = Store::new(temp_root("cap"));
        for i in 0..(MEMORY_CAP + 32) {
            store.memory_put(&format!("k{i}"), format!("v{i}").as_bytes());
        }
        assert!(store.memory_get("k31").is_none(), "oldest evicted");
        let newest = MEMORY_CAP + 31;
        assert!(store.memory_get(&format!("k{newest}")).is_some());
    }
}
