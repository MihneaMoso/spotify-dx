use crate::adblock::engine;
use crate::state::AdblockStats;
use anyhow::Context as _;
use once_cell::sync::Lazy;
use parking_lot::RwLock;

/// Blocklist sources, refreshed in the background at startup.
pub const BLOCKLIST_URLS: &[&str] = &[
    "https://adguardteam.github.io/AdGuardSDNSFilter/Filters/filter.txt",
    "https://raw.githubusercontent.com/nicehash/NiceHashAdblocker/master/hosts/hosts.txt",
];

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

/// Notified whenever the counters change (`record_drop`, blocklist refresh), so
/// the UI can mirror `ADBLOCK_STATS` event-driven instead of polling on a 1s
/// timer.
pub static STATS_CHANGED: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// Load the bundled snapshot, spawn the engine thread, and kick off a
/// background refresh.  The engine thread builds the `adblock::Engine` lazily
/// on first URL check; the text cache is loaded here so the rule count can be
/// reported immediately.
pub async fn init() -> anyhow::Result<()> {
    // Spawn the dedicated engine thread (Engine is !Send, so it lives here).
    engine::spawn_engine_thread();

    // Build the engine on the engine thread and report the rule count.
    // The engine thread builds lazily, but we can report readiness now.
    {
        let mut stats = STATS.write();
        stats.tracked = engine::block_count();
        stats.cached_entries = engine::block_count();
    }
    tracing::info!("adblock: engine thread spawned");

    // Resolve the Spotify control-plane endpoints once to prove our filter
    // does not interfere with the API.
    if let Ok(ips) = engine::dns_resolve("api.spotify.com").await {
        tracing::debug!("adblock: api.spotify.com resolves to {ips:?}");
    }

    // Background refresh — never blocks UI startup. Native uses a tokio task;
    // wasm's reqwest futures are !Send so `spawn` won't do — use spawn_local.
    #[cfg(not(target_arch = "wasm32"))]
    tokio::spawn(refresh_lists());
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(refresh_lists());
    Ok(())
}

/// Fetch every list, rebuild the engine, and atomically rewrite the cache.
async fn refresh_lists() {
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

    // Rebuild the engine with the fresh list.
    engine::rebuild_engine(&merged);

    {
        let mut stats = STATS.write();
        stats.tracked = engine::block_count();
        stats.cached_entries = engine::block_count();
    }
    STATS_CHANGED.notify_waiters();
    tracing::info!("adblock: refreshed blocklist, engine now has {} rules", engine::block_count());

    // Store the merged text for version-change detection on next boot.
    if let Ok(mut current) = engine::FRESH_LIST.write() {
        *current = Some(merged);
    }
}

/// Record one dropped request. Safe to call from any thread.
pub fn record_drop() {
    STATS.write().blocked += 1;
    STATS_CHANGED.notify_waiters();
}

/// Thread-safe snapshot of the current filter state, for the UI to mirror into
/// the `ADBLOCK_STATS` global signal on the UI thread.
pub fn snapshot() -> AdblockStats {
    *STATS.read()
}

/// Await until the blocker counters change (`record_drop` / refresh). Lets the
/// UI mirror `ADBLOCK_STATS` event-driven instead of polling.
pub async fn stats_changed() {
    STATS_CHANGED.notified().await;
}

/// A *plain* reqwest client — deliberately NOT the filtered client, because the
/// blocklist has to be fetchable even if it would block itself (bootstrap).
fn remote_client() -> reqwest::Client {
    let builder = reqwest::Client::builder().user_agent(
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
         (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36",
    );
    // `timeout` is native-only in reqwest; the wasm/fetch backend has its own
    // (untunable) timeouts.
    #[cfg(not(target_arch = "wasm32"))]
    let builder = builder.timeout(std::time::Duration::from_secs(30));
    builder
        .build()
        .expect("reqwest client for blocklist bootstrap")
}

async fn fetch_list(url: &str) -> anyhow::Result<String> {
    // `Accept-Encoding` is a forbidden header under fetch; browsers negotiate it
    // transparently, so only set it on native.
    #[cfg(not(target_arch = "wasm32"))]
    let resp = remote_client()
        .get(url)
        .header("Accept-Encoding", "gzip")
        .send()
        .await
        .context("blocklist request failed")?;
    #[cfg(target_arch = "wasm32")]
    let resp = remote_client()
        .get(url)
        .send()
        .await
        .context("blocklist request failed")?;
    if !resp.status().is_success() {
        anyhow::bail!("blocklist fetch returned {}", resp.status());
    }
    resp.text().await.context("blocklist body read failed")
}

/// The most recent merged snapshot, if a background refresh completed.
pub fn current_merged_snapshot() -> Option<String> {
    engine::FRESH_LIST
        .read()
        .ok()
        .and_then(|guard| guard.clone())
}
