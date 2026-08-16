use crate::adblock::dns_filter;
use crate::state::AdblockStats;
use anyhow::Context as _;
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use std::path::{Path, PathBuf};

/// Blocklist sources, refreshed in the background at startup.
pub const BLOCKLIST_URLS: &[&str] = &[
    "https://adguardteam.github.io/AdGuardSDNSFilter/Filters/filter.txt",
    "https://raw.githubusercontent.com/nicehash/NiceHashAdblocker/master/hosts/hosts.txt",
];

/// The bundled snapshot shipped in `assets/blocklist_cache.txt`. The snapshot is
/// always loadable, so the filter never blocks on the network for cold starts.
const CACHE_FILE_NAME: &str = "blocklist_cache.txt";

/// Thread-safe counters. Dioxus signals must only be touched on the UI thread,
/// so the background refresh thread records here instead and the UI polls this
/// snapshot into `ADBLOCK_STATS`.
static STATS: Lazy<RwLock<AdblockStats>> = Lazy::new(|| {
    RwLock::new(AdblockStats {
        tracked: 0,
        blocked: 0,
        cached_entries: 0,
        ad_fetch_failures: 0,
    })
});

static FRESH_LIST: Lazy<std::sync::RwLock<Option<String>>> =
    Lazy::new(|| std::sync::RwLock::new(None));

/// Load the cached snapshot, then kick off a background refresh.
pub async fn init() -> anyhow::Result<()> {
    // 1. Load the bundled snapshot (always succeeds when assets are present).
    let cache_path = cache_path();
    let mut cached_text = include_str!("../../assets/blocklist_cache.txt").to_owned();
    if let Ok(from_disk) = std::fs::read_to_string(&cache_path) {
        // Prefer anything newer on disk over the bundled copy.
        cached_text = from_disk;
    }
    let added = dns_filter::load_hosts_content(&cached_text);
    {
        let mut stats = STATS.write();
        stats.tracked = dns_filter::block_count();
        stats.cached_entries = added;
    }
    tracing::info!("adblock: loaded {added} cached entries");

    // 2. Resolve the Spotify control-plane endpoints once to prove our filter
    //    does not interfere with the API.
    if let Ok(ips) = dns_filter::resolve("api.spotify.com").await {
        tracing::debug!("adblock: api.spotify.com resolves to {ips:?}");
    }

    // 3. Background refresh — never blocks UI startup.
    tokio::spawn(refresh_lists(cache_path));
    Ok(())
}

/// Fetch every list, merge into the live trie and atomically rewrite the cache.
async fn refresh_lists(cache_path: PathBuf) {
    let mut merged = String::new();
    for url in BLOCKLIST_URLS {
        match fetch_list(url).await {
            Ok(body) => {
                merged.push_str(&body);
                merged.push('\n');
            }
            Err(err) => {
                STATS.write().ad_fetch_failures += 1;
                tracing::warn!("adblock: failed to fetch {url}: {err:#}");
            }
        }
    }
    if merged.trim().is_empty() {
        tracing::warn!("adblock: refresh produced no data, keeping cached snapshot");
        return;
    }

    let added = dns_filter::load_hosts_content(&merged);
    {
        let mut stats = STATS.write();
        stats.tracked = dns_filter::block_count();
    }
    tracing::info!("adblock: refreshed blocklist, {added} new entries");

    if let Ok(mut current) = FRESH_LIST.write() {
        *current = Some(merged.clone());
    }
    write_cache_atomic(&cache_path, &merged);
}

/// Record one dropped request. Safe to call from any thread.
pub fn record_drop() {
    STATS.write().blocked += 1;
}

/// Thread-safe snapshot of the current filter state, for the UI to mirror into
/// the `ADBLOCK_STATS` global signal on the UI thread.
pub fn snapshot() -> AdblockStats {
    *STATS.read()
}

/// A *plain* reqwest client — deliberately NOT the filtered client, because the
/// blocklist has to be fetchable even if it would block itself (bootstrap).
fn remote_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36",
        )
        .build()
        .expect("reqwest client for blocklist bootstrap")
}

async fn fetch_list(url: &str) -> anyhow::Result<String> {
    let resp = remote_client()
        .get(url)
        .header("Accept-Encoding", "gzip")
        .send()
        .await
        .context("blocklist request failed")?;
    if !resp.status().is_success() {
        anyhow::bail!("blocklist fetch returned {}", resp.status());
    }
    resp.text().await.context("blocklist body read failed")
}

/// Full path of the merged cache file (never the bundled asset — that stays
/// read-only inside the install directory).
fn cache_path() -> PathBuf {
    crate::util::cache_dir().join(CACHE_FILE_NAME)
}

/// Write the merged list to a temp file in the same directory, then rename.
fn write_cache_atomic(dest: &Path, content: &str) {
    let Some(dir) = dest.parent() else {
        tracing::error!("adblock: cache path has no parent directory");
        return;
    };
    if let Err(err) = std::fs::create_dir_all(dir) {
        tracing::warn!("adblock: cannot create cache dir: {err}");
        return;
    }
    let tmp = dest.with_extension("tmp");
    let result = std::fs::write(&tmp, content).and_then(|_| std::fs::rename(&tmp, dest));
    if let Err(err) = result {
        tracing::warn!("adblock: failed to persist cache: {err}");
        let _ = std::fs::remove_file(&tmp);
    }
}

/// The most recent merged snapshot, if a background refresh completed.
pub fn current_merged_snapshot() -> Option<String> {
    FRESH_LIST
        .read()
        .ok()
        .and_then(|guard| guard.clone())
}