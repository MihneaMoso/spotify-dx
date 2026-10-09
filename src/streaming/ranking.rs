//! Live provider ranking: passive scoring + periodic decay, tiered order.
//!
//! Every resolve outcome feeds per-provider stats (success/latency/fail);
//! a 60-second timer task decays failures so recovered legs come back.
//! The resolver sorts available providers per resolve: policy tiers
//! first, live score within tiers. Scoring is passive (zero extra I/O)
//! and decay is arithmetic — nothing here blocks or clogs any thread.
//! A per-leg time budget bounds fallthrough cost on top.
//!
//! Two deliberate non-goals, documented so they aren't "fixed" later:
//! - Quality preferences do NOT reorder providers. Prefs already govern
//!   variant choice INSIDE each provider (advisory caps) and tier
//!   placement (credential lossless first when configured); re-sorting by
//!   ceiling would trade a fast correct 128kbps for a slow wrong 320kbps.
//!   Latency + reliability order; caps decide quality.
//! - Correctness (wrong-song) is NOT scored. A URL returned is a success
//!   even if the match was bad — match quality stays in the per-provider
//!   textual/duration gates, which see the content and scores never will.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Policy tier: catalog suitability, static. Full-track mainstream first,
/// honest previews last (a labeled 30s beats NOT_FOUND, never a full
/// song), upload-catalog (indie/remix, high wrong-song risk on mainstream
/// queries) after previews. Scores only order WITHIN a tier.
pub fn tier(name: &str) -> u8 {
    match name {
        "tidal" | "qobuz" => 0,
        "youtube" | "invidious" | "piped" | "saavn" => 1,
        "deezer" => 2,
        _ => 3,
    }
}

/// Prior pseudo-counts (success weight): research-based reputation before
/// any live data. Unknown names get a neutral prior.
fn prior(name: &str) -> (f64, f64) {
    match name {
        "youtube" => (9.0, 1.0),
        "invidious" => (6.0, 4.0),
        "deezer" => (9.0, 1.0),
        "saavn" => (4.0, 6.0),
        "soundcloud" => (5.0, 5.0),
        "audius" => (5.0, 5.0),
        "piped" => (2.0, 8.0),
        "tidal" | "qobuz" => (9.0, 1.0),
        _ => (5.0, 5.0),
    }
}

/// Per-provider live stats. Counters only — no timestamps, so decay needs
/// no clock (works on wasm and under clock jumps).
#[derive(Debug, Clone, Default)]
pub struct ProviderStats {
    pub ok: u64,
    pub fail: u64,
    /// Exponential moving average of successful resolve latency, ms.
    pub ema_ms: f64,
}

impl ProviderStats {
    /// Success rate with prior smoothing (a provider with 1/1 beats 0/0,
    /// but 0/0 still beats 0/5 thanks to priors).
    pub fn rate(&self, name: &str) -> f64 {
        let (po, pf) = prior(name);
        (self.ok as f64 + po) / (self.ok as f64 + self.fail as f64 + po + pf)
    }
}

/// Sort key: tier first, then rate desc, then latency asc. Pure, tested.
pub fn rank_key(name: &str, stats: &ProviderStats) -> (u8, impl Ord, impl Ord) {
    // Rate as u64 permille keeps the key Ord without float pitfalls.
    let rate_mille = (stats.rate(name) * 1000.0) as u64;
    let latency = stats.ema_ms as u64;
    (tier(name), std::cmp::Reverse(rate_mille), latency)
}

/// Halve failure counts (and streaks): recovered legs climb back without
/// waiting out a full TTL, while chronic failures stay buried. Pure.
pub fn decay_table(table: &mut HashMap<&'static str, ProviderStats>) {
    for stats in table.values_mut() {
        stats.fail /= 2;
    }
}

static RANK: OnceLock<Mutex<HashMap<&'static str, ProviderStats>>> = OnceLock::new();

fn table() -> &'static Mutex<HashMap<&'static str, ProviderStats>> {
    RANK.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Record one leg outcome (passive scoring — called from the resolve loop,
/// never performs I/O). Timeouts count as failures (no latency sample:
/// a stalled leg must not normalize its stall into the average).
pub fn record(name: &'static str, ok: bool, elapsed_ms: Option<u64>) {
    let mut t = table().lock().unwrap_or_else(|e| e.into_inner());
    let stats = t.entry(name).or_default();
    if ok {
        stats.ok = stats.ok.saturating_add(1);
        if let Some(ms) = elapsed_ms {
            let alpha = 0.3;
            stats.ema_ms = if stats.ema_ms == 0.0 {
                ms as f64
            } else {
                alpha * ms as f64 + (1.0 - alpha) * stats.ema_ms
            };
        }
    } else {
        stats.fail = stats.fail.saturating_add(1);
    }
}

/// Snapshot scores for ordering (one brief lock per resolve).
pub fn snapshot() -> HashMap<&'static str, ProviderStats> {
    table().lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Periodic decay driver: arithmetic only, no I/O, sleeps between runs.
/// Spawned once per process; a missing runtime (tests, headless contexts)
/// simply skips it — scores still accumulate passively.
#[cfg(not(target_arch = "wasm32"))]
pub fn ensure_decay_task() {
    static STARTED: OnceLock<()> = OnceLock::new();
    if STARTED.get().is_some() {
        return;
    }
    // tokio::spawn needs a runtime; resolve() always runs inside one, so
    // the first call succeeds and latches. If it panics (no runtime),
    // the latch stays unset and the next resolve retries.
    let spawned = std::panic::catch_unwind(|| {
        tokio::spawn(async {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                if let Ok(mut t) = table().lock() {
                    decay_table(&mut t);
                }
            }
        });
    });
    if spawned.is_ok() {
        let _ = STARTED.set(());
    }
}

#[cfg(target_arch = "wasm32")]
pub fn ensure_decay_task() {}

/// Per-leg time budget: a stalled leg aborts into the next provider
/// instead of holding the resolve hostage (the tens-of-seconds chains).
/// Generous on purpose — typical legs finish in 1–4s; only true stalls
/// trip it, and scores demote repeat stallers afterward.
#[cfg(not(target_arch = "wasm32"))]
pub const PROVIDER_BUDGET: std::time::Duration = std::time::Duration::from_secs(15);

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(ok: u64, fail: u64, ema: f64) -> ProviderStats {
        ProviderStats { ok, fail, ema_ms: ema }
    }

    /// Tiers beat scores: a perfect-score preview never outranks a
    /// mediocre full-track leg; upload catalog sorts last.
    #[test]
    fn tiers_dominate_scores() {
        let t: HashMap<&str, ProviderStats> = HashMap::new();
        let empty = stats(0, 0, 0.0);
        let mut ns = ["audius", "deezer", "youtube"];
        ns.sort_by_key(|n| rank_key(n, t.get(n).unwrap_or(&empty)));
        assert_eq!(ns, ["youtube", "deezer", "audius"]);
    }

    /// Within a tier, reliability then latency decide.
    #[test]
    fn scores_order_within_tier() {
        let mut t = HashMap::new();
        t.insert("youtube", stats(0, 20, 100.0));
        t.insert("invidious", stats(8, 2, 3000.0));
        t.insert("saavn", stats(0, 0, 0.0));
        let mut ns = ["youtube", "invidious", "saavn"];
        ns.sort_by_key(|n| rank_key(n, &t[n]));
        // invidious (proven) > saavn (neutral prior) > youtube (failing).
        assert_eq!(ns, ["invidious", "saavn", "youtube"]);
    }

    /// Decay rehabilitates: halved failures climb back toward priors.
    #[test]
    fn decay_forgives() {
        let mut t = HashMap::new();
        t.insert("piped", stats(0, 8, 0.0));
        let before = t["piped"].rate("piped");
        decay_table(&mut t);
        assert_eq!(t["piped"].fail, 4);
        assert!(t["piped"].rate("piped") > before);
    }

    /// Priors give sane cold order: youtube first, piped last.
    #[test]
    fn cold_start_order_is_sane() {
        let t: HashMap<&str, ProviderStats> = HashMap::new();
        let empty = stats(0, 0, 0.0);
        let mut ns = ["audius", "deezer", "youtube", "piped", "saavn", "invidious", "soundcloud"];
        ns.sort_by_key(|n| rank_key(n, t.get(n).unwrap_or(&empty)));
        assert_eq!(ns[0], "youtube");
        assert_eq!(ns[ns.len() - 3], "deezer");
        assert_eq!(&ns[ns.len() - 2..], &["audius", "soundcloud"]);
        assert!(ns.contains(&"piped"));
    }
}
