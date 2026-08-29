use adblock::lists::{FilterFormat, FilterSet, ParseOptions};
use adblock::request::Request;
use adblock::Engine;
use once_cell::sync::Lazy;
use std::sync::atomic::{AtomicUsize, Ordering};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::OnceLock;

/// Hostnames that must never be blocked regardless of the blocklist content.
/// These are encoded as adblock exception rules (`@@||host^`) so the engine
/// itself enforces the whitelist.
pub const ALWAYS_ALLOW: &[&str] = &[
    "accounts.spotify.com",
    "open.spotify.com",
    "api.spotify.com",
    "spclient.wg.spotify.com",
    "apresolve.spotify.com",
    "login5.spotify.com",
    "clienttoken.spotify.com",
    "auth.spotify.com",
    "sdk.scdn.co",
    "i.scdn.co",
    "t.scdn.co",
    "mosaic.scdn.co",
    "lineup-images.scdn.co",
    "seeded-session-images.scdn.co",
    "odns.spotify.com",
];

/// Wildcard domains whose subdomains are also whitelisted.
const WILDCARD_ALLOW: &[&str] = &["spotify.com", "spotifycdn.com", "scdn.co"];

#[cfg(target_arch = "wasm32")]
const ENGINE_CACHE_FILE: &str = "adblock://engine";
#[cfg(target_arch = "wasm32")]
const BLOCKLIST_CACHE_KEY: &str = "adblock://blocklist";

/// Source hostname assumed for all outbound requests when constructing
/// `Request` objects for the blocker.
const SOURCE_HOSTNAME: &str = "open.spotify.com";

// ── Engine thread communication (native) ────────────────────────────────────
//
// `adblock::Engine` is `!Send + !Sync` (uses `Rc`/`RefCell` internally), so it
// cannot live in a `static` shared across threads. On native a dedicated std
// thread owns the Engine and checks URLs via `mpsc`. On wasm (single-threaded)
// the engine lives in a `thread_local` and is checked synchronously.

#[cfg(not(target_arch = "wasm32"))]
struct CheckRequest {
    url: String,
    reply: std::sync::mpsc::SyncSender<bool>,
}

#[cfg(not(target_arch = "wasm32"))]
static ENGINE_TX: OnceLock<std::sync::mpsc::SyncSender<CheckRequest>> = OnceLock::new();

static BLOCK_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Build the engine lazily on first need (native thread / wasm main thread),
/// loading the serialized cache or compiling the bundled/fresh blocklist.
fn build_or_load_engine() -> Option<Engine> {
    // 1. Try the serialized engine cache (fast restart).
    #[cfg(not(target_arch = "wasm32"))]
    if let Ok(bytes) = std::fs::read(engine_path()) {
        let mut engine = Engine::default();
        if engine.deserialize(&bytes).is_ok() {
            tracing::info!("adblock: loaded cached engine");
            return Some(engine);
        }
        tracing::warn!("adblock: cached engine corrupted, rebuilding");
    }
    #[cfg(target_arch = "wasm32")]
    if let Some(bytes) = crate::platform::storage::get_bytes(ENGINE_CACHE_FILE) {
        let mut engine = Engine::default();
        if engine.deserialize(&bytes).is_ok() {
            tracing::info!("adblock: loaded cached engine from storage");
            return Some(engine);
        }
        tracing::warn!("adblock: cached engine corrupted, rebuilding");
    }

    // 2. Build from the blocklist text (bundled, or fresher persisted copy).
    #[cfg(not(target_arch = "wasm32"))]
    let blocklist = {
        let mut text = include_str!("../../assets/blocklist_cache.txt").to_owned();
        if let Ok(from_disk) = std::fs::read_to_string(cache_path()) {
            text = from_disk;
        }
        text
    };
    #[cfg(target_arch = "wasm32")]
    let blocklist = {
        let mut text = include_str!("../../assets/blocklist_cache.txt").to_owned();
        if let Some(bytes) = crate::platform::storage::get_bytes(BLOCKLIST_CACHE_KEY) {
            if let Ok(s) = String::from_utf8(bytes) {
                text = s;
            }
        }
        text
    };

    let (engine, rule_count) = build_engine_with_count(&blocklist);
    BLOCK_COUNT.store(rule_count, Ordering::Relaxed);
    tracing::info!("adblock: compiled engine with {rule_count} rules from blocklist");

    #[cfg(not(target_arch = "wasm32"))]
    save_engine_cache(&engine);
    #[cfg(target_arch = "wasm32")]
    save_engine_cache_to_storage(&engine);
    Some(engine)
}

// ── Engine thread lifecycle (native) ────────────────────────────────────────

/// Spawn the dedicated engine thread.  Must be called once during
/// `adguard_api::init()`.  The thread owns the `Engine` (which is `!Send`)
/// and processes URL check requests over an `mpsc` channel. No-op on wasm.
#[cfg(not(target_arch = "wasm32"))]
pub fn spawn_engine_thread() {
    let (tx, rx) = std::sync::mpsc::sync_channel(64);

    std::thread::Builder::new()
        .name("adblock-engine".into())
        .spawn(move || engine_thread(rx))
        .expect("failed to spawn adblock engine thread");

    ENGINE_TX.set(tx).expect("adblock engine thread already spawned");
    tracing::info!("adblock: engine thread spawned");
}

#[cfg(target_arch = "wasm32")]
pub fn spawn_engine_thread() {
    // Nothing to spawn: the engine is built lazily and checked in-line on the
    // single wasm thread. Mark ready so `is_ready()` is honest about lifecycle.
    tracing::info!("adblock: engine runs in-line on wasm (no thread)");
}

#[cfg(not(target_arch = "wasm32"))]
fn engine_thread(rx: std::sync::mpsc::Receiver<CheckRequest>) {
    let mut engine: Option<Engine> = None;

    for req in rx {
        if engine.is_none() {
            engine = build_or_load_engine();
        }
        let Some(ref eng) = engine else {
            let _ = req.reply.send(false);
            continue;
        };

        let Ok(request) = Request::new(&req.url, SOURCE_HOSTNAME, "xhr", "GET") else {
            let _ = req.reply.send(false);
            continue;
        };

        let _ = req.reply.send(eng.check_network_request(&request).should_block());
    }
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Build a fresh `FilterSet` from blocklist content and exception rules, then
/// compile it into an `Engine`.  Returns the engine and the number of rules.
pub fn build_engine(blocklist_text: &str) -> Engine {
    let (engine, _count) = build_engine_with_count(blocklist_text);
    engine
}

fn build_engine_with_count(blocklist_text: &str) -> (Engine, usize) {
    let mut filter_set = FilterSet::new(false);

    // 1. Load the blocklist content.  Split by format: AdGuard `||...^` rules
    //    use `Standard` format; hosts `0.0.0.0 hostname` and bare hostnames use
    //    `Hosts` format.  Mixing them in one `add_filter_list` call silently
    //    drops the non-matching format.
    let (adguard_text, hosts_text) = split_blocklist_formats(blocklist_text);
    if !hosts_text.is_empty() {
        filter_set.add_filter_list(hosts_text, ParseOptions { format: FilterFormat::Hosts, ..Default::default() });
    }
    if !adguard_text.is_empty() {
        filter_set.add_filter_list(adguard_text, ParseOptions::default());
    }

    // 2. Add curated Spotify ad/analytics rules (RESEARCH §3.3).
    let mut spotify_ad_rules = String::new();
    for rule in [
        "||doubleclick.net^",
        "||googlesyndication.com^",
        "||fastly-insights.com^",
        "||sentry.io^",
        "||googleadservices.com^",
        "||googletagmanager.com^",
        "||google-analytics.com^",
        "||facebook.com/tr^",
        "||hotjar.com^",
        "||amplitude.com^",
        "||mixpanel.com^",
        "||segment.io^",
        "||branch.io^",
    ] {
        spotify_ad_rules.push_str(rule);
        spotify_ad_rules.push('\n');
    }
    filter_set.add_filter_list(spotify_ad_rules, ParseOptions::default());

    // 3. Add ALWAYS_ALLOW as exception rules so the engine itself enforces the
    //    whitelist — no secondary check needed at the call site.
    let mut exceptions = String::new();
    for domain in ALWAYS_ALLOW {
        exceptions.push_str(&format!("@@||{domain}^\n"));
    }
    for domain in WILDCARD_ALLOW {
        exceptions.push_str(&format!("@@||{domain}^\n"));
    }
    filter_set.add_filter_list(exceptions, ParseOptions::default());

    let rule_count = count_filter_lines(blocklist_text)
        + 13 // curated ad rules
        + ALWAYS_ALLOW.len()
        + WILDCARD_ALLOW.len();

    let engine = Engine::new_with_filter_set(filter_set);
    (engine, rule_count)
}

/// Split a mixed-format blocklist into AdGuard (`||...^`) lines and hosts-format
/// (`0.0.0.0 hostname` / bare hostname) lines.
fn split_blocklist_formats(text: &str) -> (String, String) {
    let mut adguard = String::new();
    let mut hosts = String::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('!') {
            continue;
        }
        // AdGuard rules: `||domain^`, `@@||domain^`, or contain `$` options
        if trimmed.starts_with("||") || trimmed.starts_with("@@") || trimmed.contains('$') {
            adguard.push_str(trimmed);
            adguard.push('\n');
        } else {
            hosts.push_str(trimmed);
            hosts.push('\n');
        }
    }
    (adguard, hosts)
}

fn count_filter_lines(text: &str) -> usize {
    text.lines()
        .filter(|l| {
            let t = l.trim();
            !t.is_empty() && !t.starts_with('#') && !t.starts_with('!')
        })
        .count()
}

// ── Engine persistence (native) ─────────────────────────────────────────────

/// Full path of the serialized engine cache (native only).
#[cfg(not(target_arch = "wasm32"))]
fn engine_path() -> std::path::PathBuf {
    crate::util::data_dir().join("adblock_engine.bin")
}

#[cfg(not(target_arch = "wasm32"))]
fn cache_path() -> std::path::PathBuf {
    crate::util::data_dir().join("blocklist_cache.txt")
}

/// Save the engine binary cache to disk (native).
#[cfg(not(target_arch = "wasm32"))]
fn save_engine_cache(engine: &Engine) {
    let path = engine_path();
    let Some(dir) = path.parent() else {
        tracing::error!("adblock: engine cache path has no parent directory");
        return;
    };
    if let Err(err) = std::fs::create_dir_all(dir) {
        tracing::warn!("adblock: cannot create cache dir: {err}");
        return;
    }
    let bytes = engine.serialize();
    let tmp = path.with_extension("tmp");
    let result = std::fs::write(&tmp, &bytes).and_then(|_| std::fs::rename(&tmp, &path));
    if let Err(err) = result {
        tracing::warn!("adblock: failed to persist engine cache: {err}");
        let _ = std::fs::remove_file(&tmp);
    }
}

/// Save the engine binary cache to storage (wasm).
#[cfg(target_arch = "wasm32")]
fn save_engine_cache_to_storage(engine: &Engine) {
    let bytes = engine.serialize();
    crate::platform::storage::set_bytes(ENGINE_CACHE_FILE, &bytes);
}

// ── In-line engine (wasm) ───────────────────────────────────────────────────

#[cfg(target_arch = "wasm32")]
thread_local! {
    /// The compiled engine, owned by the single wasm thread.
    static WASM_ENGINE: std::cell::RefCell<Option<Engine>> = const {
        std::cell::RefCell::new(None)
    };
}

/// Check whether a URL should be blocked.  Safe to call from any thread.
///
/// Native: sends the URL to the engine thread and waits for a reply.  Wasm: the
/// engine is built lazily on first use and checked in-line.  Either way a
/// missing engine (init not called) returns `false` (fail-open).
pub fn should_block_url(url: &str) -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let Some(tx) = ENGINE_TX.get() else {
            return false;
        };
        let (reply_tx, reply_rx) = std::sync::mpsc::sync_channel(1);
        if tx
            .send(CheckRequest { url: url.to_owned(), reply: reply_tx })
            .is_err()
        {
            return false;
        }
        reply_rx.recv().unwrap_or(false)
    }
    #[cfg(target_arch = "wasm32")]
    {
        WASM_ENGINE.with(|cell| {
            let mut eng = cell.borrow_mut();
            if eng.is_none() {
                *eng = build_or_load_engine();
            }
            let Some(engine) = eng.as_ref() else {
                return false;
            };
            let Ok(request) = Request::new(url, SOURCE_HOSTNAME, "xhr", "GET") else {
                return false;
            };
            engine.check_network_request(&request).should_block()
        })
    }
}

/// Whether the engine has been started/initialised.
pub fn is_ready() -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        ENGINE_TX.get().is_some()
    }
    #[cfg(target_arch = "wasm32")]
    {
        WASM_ENGINE.with(|c| !c.borrow().is_none())
    }
}

/// Number of compiled rules in the engine.  Updated after each build/cache
/// load.  A value of `0` before the first check is normal.
pub fn block_count() -> usize {
    BLOCK_COUNT.load(Ordering::Relaxed)
}

/// Rebuild the engine from fresh blocklist text (called by `adguard_api` after
/// a background refresh).  Builds and caches for next boot; the running engine
/// picks up the new rules on its next lazy init.
pub fn rebuild_engine(blocklist_text: &str) {
    let (engine, rule_count) = build_engine_with_count(blocklist_text);
    BLOCK_COUNT.store(rule_count, Ordering::Relaxed);
    #[cfg(not(target_arch = "wasm32"))]
    save_engine_cache(&engine);
    #[cfg(target_arch = "wasm32")]
    save_engine_cache_to_storage(&engine);
    tracing::info!("adblock: engine rebuilt with {rule_count} rules, cached for next boot");
}

/// DNS-over-HTTPS resolver using Cloudflare (`1.1.1.1/dns-query`). Native only.
#[cfg(not(target_arch = "wasm32"))]
pub async fn dns_resolve(
    host: &str,
) -> Result<Vec<std::net::IpAddr>, anyhow::Error> {
    use hickory_resolver::config::{ResolverConfig, ResolverOpts};
    use hickory_resolver::TokioAsyncResolver;

    let resolver =
        TokioAsyncResolver::tokio(ResolverConfig::cloudflare_https(), ResolverOpts::default());
    let resp = resolver.lookup_ip(host).await?;
    let ips: Vec<_> = resp.iter().collect();
    if ips.is_empty() {
        anyhow::bail!("no A/AAAA records for {host}");
    }
    Ok(ips)
}

/// DNS-over-HTTPS is a native bootstrap diagnostic; wasm relies on the browser's
/// resolver, so it is a no-op error (callers treat it as best-effort).
#[cfg(target_arch = "wasm32")]
pub async fn dns_resolve(_host: &str) -> Result<Vec<std::net::IpAddr>, anyhow::Error> {
    anyhow::bail!("DNS-over-HTTPS is not used on wasm")
}

/// The most recent merged blocklist text, stored after a background refresh
/// completes.  Used to detect list-version bumps.
pub static FRESH_LIST: Lazy<std::sync::RwLock<Option<String>>> =
    Lazy::new(|| std::sync::RwLock::new(None));

// ── Tests (run on the main thread where Engine is valid) ───────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_engine_blocks_known_ad_domain() {
        let list = "0.0.0.0 ads.doubleclick.net\n0.0.0.0 tracker.example.com\n";
        let engine = build_engine(list);

        let req = Request::new(
            "https://ads.doubleclick.net/some/path?q=1",
            SOURCE_HOSTNAME,
            "xhr",
            "GET",
        )
        .unwrap();
        assert!(engine.check_network_request(&req).should_block());

        let req2 = Request::new(
            "https://tracker.example.com/collect",
            SOURCE_HOSTNAME,
            "xhr",
            "GET",
        )
        .unwrap();
        assert!(engine.check_network_request(&req2).should_block());
    }

    #[test]
    fn build_engine_allows_spotify_domains() {
        let list = "0.0.0.0 ads.spotify.com\n||doubleclick.net^\n";
        let engine = build_engine(list);

        let urls = [
            "https://ads.spotify.com/some/path?q=1",
            "https://api.spotify.com/v1/me",
            "https://accounts.spotify.com/authorize",
            "https://open.spotify.com/browse",
            "https://i.scdn.co/image/abc",
            "https://audio-ak.spotifycdn.com/track.mp4",
            "https://spclient.wg.spotify.com/playlist/v3/xyz",
        ];

        for url in &urls {
            let req = Request::new(url, SOURCE_HOSTNAME, "xhr", "GET").unwrap();
            assert!(
                !engine.check_network_request(&req).should_block(),
                "must not block {url}"
            );
        }
    }

    #[test]
    fn exception_rules_whitelist_always_allow() {
        let list = "||accounts.spotify.com^\n||open.spotify.com^\n";
        let engine = build_engine(list);

        let req = Request::new(
            "https://accounts.spotify.com/en/login",
            SOURCE_HOSTNAME,
            "document",
            "GET",
        )
        .unwrap();
        assert!(
            !engine.check_network_request(&req).should_block(),
            "ALWAYS_ALLOW exception must override block rule"
        );
    }

    #[test]
    fn engine_serialize_roundtrip() {
        let list = "0.0.0.0 ads.example.com\n";
        let mut engine = build_engine(list);

        let bytes = engine.serialize();
        let mut restored = Engine::default();
        restored.deserialize(&bytes).expect("deserialize");

        let req = Request::new(
            "https://ads.example.com/pixel.gif",
            SOURCE_HOSTNAME,
            "xhr",
            "GET",
        )
        .unwrap();
        assert!(restored.check_network_request(&req).should_block());
    }

    #[test]
    fn hosts_format_entries_recognized() {
        let list = "\
0.0.0.0 ad.tech.example.com
127.0.0.1 pixel.tracker.example.net
||banner.adserver.example.org^
||adserver.example.org^$third-party
";
        let engine = build_engine(list);

        let blocked = [
            "https://ad.tech.example.com/serve",
            "https://pixel.tracker.example.net/collect",
            "https://banner.adserver.example.org/ad.gif",
            "https://sub.adserver.example.org/track",
        ];
        for url in &blocked {
            let req = Request::new(url, SOURCE_HOSTNAME, "xhr", "GET").unwrap();
            assert!(
                engine.check_network_request(&req).should_block(),
                "must block {url}"
            );
        }
    }

    #[test]
    fn bench_engine_lookups() {
        let mut list = String::new();
        for i in 0..5_000 {
            list.push_str(&format!("0.0.0.0 tracker{i}.example{i}.com\n"));
        }
        let engine = build_engine(&list);

        let start = std::time::Instant::now();
        let mut hits = 0u32;
        for i in 0..1_000_000 {
            let url = format!(
                "https://sub.tracker{}.example{}.com/collect",
                i % 5_000,
                i % 5_000
            );
            let req = Request::new(&url, SOURCE_HOSTNAME, "xhr", "GET").unwrap();
            if engine.check_network_request(&req).should_block() {
                hits += 1;
            }
        }
        let elapsed = start.elapsed();
        assert_eq!(hits, 1_000_000, "all generated needles must hit");
        assert!(
            elapsed.as_secs_f64() < 10.0,
            "1M engine lookups took {elapsed:?}, expected < 10s",
        );
    }
}
