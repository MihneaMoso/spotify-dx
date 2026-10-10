//! Invidious provider: the same YouTube catalog through independent
//! front-ends, with server-side proxied audio.
//!
//! Direct InnerTube adaptive URLs carry `gir=yes` and are unusable to us
//! (transfer budget ~1MB then hard 403s — see `youtube::is_throttled_url`).
//! Invidious `local=true` URLs are fetched by the INSTANCE server-side and
//! re-served from the instance itself, so our client never faces the
//! throttle wall — and the bytes are plain cacheable audio. It sits AFTER
//! `youtube` in the chain: direct clean URLs (muxed) stay first; Invidious
//! only runs when they fail, costing nothing otherwise.
//!
//! Flow per healthy instance, in order:
//! 1. `GET {api}/api/v1/search?q={artist} {title}&type=video` → video IDs.
//! 2. `GET {api}/api/v1/videos/{id}?local=true&fields=...` → proxied
//!    adaptive formats; prefer Opus itags 251 → 250 → 249 within the
//!    quality cap (honest degrade beyond it, same doctrine as YouTube).
//! 3. Byte-level media probe (`Range: bytes=0-1023` → 206 + `audio/*`)
//!    before trusting the URL — API 200s do NOT prove media flows (bot
//!    walls serve challenge pages with 200s).
//!
//! Instance health is tracked in-process like Piped (consecutive transport
//! failures cool an instance; `is_available` is false only when every
//! instance is cooling). The pool refreshes best-effort from the public
//! instance list; refresh NEVER shrinks the pool and fails silently, so a
//! dead list endpoint costs nothing. A short client timeout bounds dead
//! hosts. Proxied URLs are instance-bound and expire (~6h), so they are
//! re-resolved per play like every other upstream URL (never persisted
//! beyond the in-memory upstream map).

use std::collections::HashMap;
use std::sync::Mutex;
#[cfg(not(target_arch = "wasm32"))]
use std::time::{Duration, Instant};

use async_trait::async_trait;

use super::common::{fanout_queries, title_matches, urlencode, Fetch};
use super::youtube;
use crate::streaming::provider::{Provider, Quality, Resolution, TrackQuery};

/// Pinned Invidious API hosts (verified API-open in Oct 2026; media
/// servability is decided per-instance by the byte probe, never assumed).
/// f5.si first: the only clearnet instance with api+open in the Oct 2026
/// instances.json sweep (flokinet/nadeko/tiekoetter all api:false).
/// Shared pool: seeds are the fallback everything else degrades to.
const SEED_INSTANCES: &[&str] = &[
    "https://invidious.f5.si",
    "https://invidious.flokinet.to",
    "https://inv.nadeko.net",
    "https://invidious.tiekoetter.com",
];

/// Public instance list for best-effort pool refresh.
const INSTANCES_JSON: &str = "https://api.invidious.io/instances.json?sort_by=health";

/// Consecutive transport failures before an instance cools down.
const COOL_AFTER_FAILURES: u32 = 2;
/// Cooldown for an unhealthy instance.
#[cfg(not(target_arch = "wasm32"))]
const COOLDOWN: Duration = Duration::from_secs(5 * 60);
/// wasm has no `Instant`-based cooldown state, so recovery rides on
/// call-count probation instead (same pattern as Piped).
#[cfg(target_arch = "wasm32")]
const PROBATION_CALLS: u64 = 64;
/// Per-request budget: dead hosts fail fast instead of stalling the chain.
#[cfg(not(target_arch = "wasm32"))]
/// Per-request timeout: dead instances must fail fast (a 4-seed pool at
/// 8s each is most of a Curve-style hang; cooldowns only help within a
/// process lifetime, restarts repay everything).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// Browsers-ish UA: instances bolt on anti-bot walls against default
/// library UAs; a platform UA passes where `reqwest/x.y` does not.
const UA: &str = "Mozilla/5.0 (Linux; Android 11) AppleWebKit/537.36 Chrome/131.0 Safari/537.36";

#[derive(Default)]
struct InstanceHealth {
    consecutive_failures: u32,
    #[cfg(not(target_arch = "wasm32"))]
    cooling_until: Option<Instant>,
    #[cfg(target_arch = "wasm32")]
    checks: u64,
    #[cfg(target_arch = "wasm32")]
    cooled_at: u64,
}

pub struct InvidiousProvider {
    client: reqwest::Client,
    health: Mutex<HashMap<String, InstanceHealth>>,
    /// Owned pool (seeds + refreshed) so discovery can extend it.
    pool: Mutex<Vec<String>>,
}

impl Default for InvidiousProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl InvidiousProvider {
    pub fn new() -> Self {
        Self {
            client: {
                let builder = reqwest::Client::builder().user_agent(UA);
                #[cfg(not(target_arch = "wasm32"))]
                let builder = builder.timeout(REQUEST_TIMEOUT);
                builder.build().unwrap_or_default()
            },
            health: Mutex::new(HashMap::new()),
            pool: Mutex::new(SEED_INSTANCES.iter().map(|s| s.to_string()).collect()),
        }
    }

    /// Healthy pool members in pool order (cooling ones skipped).
    fn healthy(&self) -> Vec<String> {
        #[cfg(not(target_arch = "wasm32"))]
        let now = Instant::now();
        let mut health = self.health.lock().unwrap_or_else(|e| e.into_inner());
        let pool = self.pool.lock().unwrap_or_else(|e| e.into_inner()).clone();
        pool.into_iter()
            .filter(|api| match health.get_mut(api) {
                None => true,
                Some(h) => {
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        if h.cooling_until.map(|u| now >= u).unwrap_or(false) {
                            // Expired cooldown: forgive and retry.
                            *h = InstanceHealth::default();
                            true
                        } else if h.cooling_until.is_some() {
                            false
                        } else {
                            h.consecutive_failures < COOL_AFTER_FAILURES
                        }
                    }
                    #[cfg(target_arch = "wasm32")]
                    {
                        h.checks = h.checks.saturating_add(1);
                        if h.consecutive_failures < COOL_AFTER_FAILURES {
                            true
                        } else if h.checks.saturating_sub(h.cooled_at) >= PROBATION_CALLS {
                            *h = InstanceHealth::default();
                            true
                        } else {
                            false
                        }
                    }
                }
            })
            .collect()
    }

    fn note_success(&self, api: &str) {
        if let Ok(mut health) = self.health.lock() {
            health.remove(api);
        }
    }

    fn note_failure(&self, api: &str) {
        if let Ok(mut health) = self.health.lock() {
            let h = health.entry(api.to_string()).or_default();
            h.consecutive_failures += 1;
            #[cfg(not(target_arch = "wasm32"))]
            if h.consecutive_failures >= COOL_AFTER_FAILURES {
                h.cooling_until = Some(Instant::now() + COOLDOWN);
            }
            #[cfg(target_arch = "wasm32")]
            if h.consecutive_failures >= COOL_AFTER_FAILURES && h.cooled_at == 0 {
                h.cooled_at = h.checks;
            }
        }
    }

    /// Best-effort pool refresh from the public instance list. Additive
    /// ONLY (api:true entries join the pool tail); any failure — network,
    /// shape change, empty result — leaves the pool untouched. Called when
    /// the pool is exhausted, so discovery costs nothing on the happy path.
    async fn refresh_pool(&self) {
        let resp = match self.client.get(INSTANCES_JSON).send().await {
            Ok(r) => r,
            Err(_) => return,
        };
        let val: serde_json::Value = match resp.json().await {
            Ok(v) => v,
            Err(_) => return,
        };
        let domains = parse_instances_json(&val);
        if domains.is_empty() {
            return;
        }
        if let Ok(mut pool) = self.pool.lock() {
            for d in domains {
                if !pool.iter().any(|p| p == &d) {
                    pool.push(d);
                }
            }
        }
    }

    /// Search across query variants (accumulated to 5): transport failure
    /// vs content gap distinguished so dead hosts cool down while
    /// live-but-empty hosts don't get blamed.
    async fn search_ids(&self, api: &str, query: &TrackQuery) -> SearchOutcome {
        let queries = search_queries(&query.title, &query.artist).to_vec();
        let (ids, aborted) = fanout_queries(
            &queries,
            5,
            |s: &String| s.clone(),
            |q| async {
                match self.search_ids_one(api, q).await {
                    SearchOutcome::Hit(v) => Fetch::Items(v),
                    SearchOutcome::Miss => Fetch::Items(Vec::new()),
                    SearchOutcome::TransportFail => Fetch::Abort,
                }
            },
        )
        .await;
        if aborted {
            return SearchOutcome::TransportFail;
        }
        if ids.is_empty() {
            SearchOutcome::Miss
        } else {
            SearchOutcome::Hit(ids)
        }
    }

    /// One variant fetch: Hit ids, Miss on content gap, TransportFail when
    /// the host is dead/gated/unparseable (cools down).
    async fn search_ids_one(&self, api: &str, q: &str) -> SearchOutcome {
        let url = format!(
            "{api}/api/v1/search?q={}&type=video&fields=type,title,videoId,author",
            urlencode(q)
        );
        let resp = match self.client.get(&url).send().await {
            Ok(r) => r,
            Err(_) => return SearchOutcome::TransportFail,
        };
        if !resp.status().is_success() {
            // Disabled/gated API (403/401/500): a gated API never serves
            // THIS request; treat as transport to cool it promptly.
            return SearchOutcome::TransportFail;
        }
        let val: serde_json::Value = match resp.json().await {
            Ok(v) => v,
            Err(_) => return SearchOutcome::TransportFail,
        };
        let mut ids = Vec::new();
        for item in val.as_array().cloned().unwrap_or_default() {
            if let Some(id) = parse_search_id(&item) {
                ids.push(id);
                if ids.len() >= 5 {
                    break;
                }
            }
        }
        if ids.is_empty() {
            SearchOutcome::Miss
        } else {
            SearchOutcome::Hit(ids)
        }
    }

    /// Streams lookup for one video: proxied adaptive formats → best Opus
    /// pick within cap → byte-probed. Returns the pick on success.
    async fn streams_pick(
        &self,
        api: &str,
        video_id: &str,
        query: &TrackQuery,
    ) -> StreamsOutcome {
        let url = format!(
            "{api}/api/v1/videos/{video_id}?local=true&fields=videoId,title,author,lengthSeconds,adaptiveFormats"
        );
        let resp = match self.client.get(&url).send().await {
            Ok(r) => r,
            Err(_) => return StreamsOutcome::TransportFail,
        };
        if !resp.status().is_success() {
            return StreamsOutcome::TransportFail;
        }
        let val: serde_json::Value = match resp.json().await {
            Ok(v) => v,
            Err(_) => return StreamsOutcome::TransportFail,
        };
        // Duration gate (shared rule): a mismatched upload is worse than none.
        // Duration-less entries are rejected when our metadata has a
        // duration (same hole YouTube-direct closed: duration-less uploads
        // played an 8:45 video for a 3:32 track).
        let secs = val.get("lengthSeconds").and_then(as_u64);
        if !youtube::has_usable_duration(query.duration_ms, secs) {
            return StreamsOutcome::Miss;
        }
        if !youtube::duration_accepts(query.duration_ms, secs) {
            return StreamsOutcome::Miss;
        }
        // Textual gate: same-artist same-length genre clusters sail
        // through duration checks (observed: remix, "OU, OU", and a
        // same-artist different song all ranking for one query), so the
        // title must actually name the track. Spelling drift ("curve" vs
        // "c*rve") is forgiven only on tight durations — the fuzzy gate
        // never flies blind. Biased strict — a false reject just tries
        // the next video, a false accept plays the wrong song.
        let title = val.get("title").and_then(|t| t.as_str()).unwrap_or("");
        let author = val.get("author").and_then(|a| a.as_str()).unwrap_or("");
        let gate = if super::common::duration_tight(query.duration_ms, secs) {
            super::common::title_matches_fuzzy(&query.title, &query.artist, title, author)
        } else {
            title_matches(&query.title, &query.artist, title, author)
        };
        if !gate {
            return StreamsOutcome::Miss;
        }
        let pick = match pick_proxied_audio(&val, query.max_quality) {
            Some(p) => p,
            None => return StreamsOutcome::Miss,
        };
        // Byte-level media probe: API 200s do NOT prove media flows.
        if !self.probe_media(api, &pick.url).await {
            return StreamsOutcome::TransportFail;
        }
        StreamsOutcome::Hit(pick)
    }

    /// Range-probe a picked URL: 206 (or 200) + `audio/*` content type.
    /// Anything else (challenge pages, 403s, wrong types) fails the host.
    async fn probe_media(&self, api: &str, url: &str) -> bool {
        let resp = match self.client.get(url).header("Range", "bytes=0-1023").send().await {
            Ok(r) => r,
            Err(_) => {
                self.note_failure(api);
                return false;
            }
        };
        let ok = (resp.status().is_success()
            || resp.status().as_u16() == 206)
            && resp
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(|ct| ct.starts_with("audio/"))
                .unwrap_or(false);
        if ok {
            self.note_success(api);
        } else {
            self.note_failure(api);
        }
        ok
    }
}

/// Search outcome per instance: content gaps don't blame the host.
enum SearchOutcome {
    Hit(Vec<String>),
    /// Live host, nothing acceptable (region variance — try next).
    Miss,
    /// Dead/unparseable host (cools down).
    TransportFail,
}

/// Streams outcome per (instance, video).
enum StreamsOutcome {
    Hit(AudioPick),
    /// Live host, no usable audio (gap — try next video/instance).
    Miss,
    /// Dead host or unprovable media (cools down).
    TransportFail,
}

/// A servable proxied audio pick.
struct AudioPick {
    url: String,
    bitrate: u64,
    mime: String,
}

/// Search query variants, rank order (same shape as the direct provider:
/// legacy query first, sanitized bare-words fallback last — punctuation
/// blinds search, so the stripped form rides as the final variant).
/// Pure (unit-tested).
fn search_queries(title: &str, artist: &str) -> [String; 4] {
    [
        format!("{artist} {title} audio"),
        format!("{title} {artist}"),
        format!("{title} {artist} topic"),
        super::common::sanitize_query(&format!("{artist} {title}")),
    ]
}

/// Extract a video ID from an Invidious search item. Non-video items and
/// malformed IDs are skipped, never fatal. Pure (unit-tested).
fn parse_search_id(item: &serde_json::Value) -> Option<String> {
    if item.get("type").and_then(|t| t.as_str()) != Some("video") {
        return None;
    }
    let id = item.get("videoId").and_then(|v| v.as_str())?;
    if id.len() == 11 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        Some(id.to_string())
    } else {
        None
    }
}

/// Lenient u64 over Invidious' int-or-string numbers. Pure (unit-tested).
fn as_u64(v: &serde_json::Value) -> Option<u64> {
    if let Some(n) = v.as_u64() {
        return Some(n);
    }
    v.as_str()?.trim().parse::<u64>().ok()
}

/// Best proxied audio pick from an Invidious `videos/:id` payload:
/// Opus itags 251 → 250 → 249 first, then best audio bitrate — within the
/// cap when possible, honest degrade beyond it (same doctrine as YouTube).
/// Throttled (`gir=yes`) URLs are excluded (a misconfigured instance may
/// return raw googlevideo links); entries without URLs are skipped (the
/// cipher-recovery path belongs to YouTube-direct, not here). Pure.
fn pick_proxied_audio(val: &serde_json::Value, cap: Option<Quality>) -> Option<AudioPick> {
    let formats = val.get("adaptiveFormats")?.as_array()?;
    // Opus itag rank wins over raw bitrate (a 50k opus rivals 128k mp3).
    let rank = |itag: Option<i64>| match itag {
        Some(251) => 3,
        Some(250) => 2,
        Some(249) => 1,
        _ => 0,
    };
    let ceiling: Option<u64> = match cap {
        Some(Quality::Low) => Some(127_999),
        Some(Quality::Normal) => Some(255_999),
        _ => None,
    };
    let mut cands: Vec<(i64, u64, String, String)> = Vec::new();
    for f in formats {
        let url = match f.get("url").and_then(|u| u.as_str()) {
            Some(u) if !u.is_empty() && !youtube::is_throttled_url(u) => u.to_string(),
            _ => continue,
        };
        let mime = f.get("type").and_then(|m| m.as_str()).unwrap_or("");
        if !mime.starts_with("audio/") {
            continue;
        }
        let itag = f.get("itag").and_then(as_u64).map(|i| i as i64);
        let bitrate = f.get("bitrate").and_then(as_u64).unwrap_or(0);
        cands.push((itag.unwrap_or(-1), bitrate, url, mime.to_string()));
    }
    if cands.is_empty() {
        return None;
    }
    // Within cap first (opus rank, then bitrate); else honest degrade.
    let in_cap: Vec<_> = match ceiling {
        Some(c) => cands.iter().filter(|(_, b, _, _)| *b <= c).collect(),
        None => cands.iter().collect(),
    };
    let pool: Vec<_> = if in_cap.is_empty() {
        cands.iter().collect()
    } else {
        in_cap
    };
    pool.into_iter()
        .max_by_key(|(itag, bitrate, _, _)| (rank(Some(*itag).filter(|i| *i >= 0)), *bitrate))
        .map(|(_, bitrate, url, mime)| AudioPick {
            url: url.clone(),
            bitrate: *bitrate,
            mime: mime.clone(),
        })
}

/// Parse the public instance list: additive domains with `api: true`
/// only; anything else (dead endpoint, shape change, empty) yields empty
/// and the pool stays untouched. Pure (unit-tested).
fn parse_instances_json(val: &serde_json::Value) -> Vec<String> {
    let arr = match val.as_array() {
        Some(a) => a,
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    for entry in arr {
        let pair = match entry.as_array() {
            Some(p) if p.len() >= 2 => p,
            _ => continue,
        };
        let domain = match pair[0].as_str() {
            Some(d) if !d.is_empty() => d,
            _ => continue,
        };
        let api = pair[1]
            .get("api")
            .and_then(|a| a.as_bool())
            .unwrap_or(false);
        if !api {
            continue;
        }
        let url = if domain.starts_with("http") {
            domain.to_string()
        } else {
            format!("https://{domain}")
        };
        out.push(url);
    }
    out
}

#[async_trait(?Send)]
impl Provider for InvidiousProvider {
    fn name(&self) -> &'static str {
        "invidious"
    }

    fn is_available(&self) -> bool {
        !self.healthy().is_empty()
    }

    async fn resolve(&self, query: &TrackQuery) -> Resolution {
        let mut pool = self.healthy();
        if pool.is_empty() {
            // Pool exhausted: attempt discovery once before giving up.
            self.refresh_pool().await;
            pool = self.healthy();
            if pool.is_empty() {
                return Resolution::Error("no healthy Invidious instance".into());
            }
        }
        // Per instance: search → videos, first fully servable pick wins.
        // Transport failures blame the host; content gaps move on silently.
        for api in &pool {
            let ids = match self.search_ids(api, query).await {
                SearchOutcome::Hit(ids) => ids,
                SearchOutcome::Miss => continue,
                SearchOutcome::TransportFail => {
                    self.note_failure(api);
                    continue;
                }
            };
            for id in &ids {
                match self.streams_pick(api, id, query).await {
                    StreamsOutcome::Hit(pick) => {
                        return Resolution::Success {
                            url: pick.url,
                            format: youtube::format_for_mime(&pick.mime),
                            quality: youtube::quality_for_bitrate(pick.bitrate),
                        };
                    }
                    StreamsOutcome::Miss => continue,
                    StreamsOutcome::TransportFail => break,
                }
            }
        }
        Resolution::NotFound
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_queries_rank_legacy_first() {
        let qs = search_queries("Mercy", "Kanye West");
        assert_eq!(qs[0], "Kanye West Mercy audio");
        assert_eq!(qs[1], "Mercy Kanye West");
        assert_eq!(qs[2], "Mercy Kanye West topic");
        assert_eq!(qs[3], "kanye west mercy");
    }

    #[test]
    fn parse_search_id_accepts_videos_only() {
        let v = serde_json::json!({ "type": "video", "videoId": "dQw4w9WgXcQ", "title": "x" });
        assert_eq!(parse_search_id(&v).as_deref(), Some("dQw4w9WgXcQ"));
        let playlist = serde_json::json!({ "type": "playlist", "videoId": "dQw4w9WgXcQ" });
        assert!(parse_search_id(&playlist).is_none());
        let short = serde_json::json!({ "type": "video", "videoId": "abc" });
        assert!(parse_search_id(&short).is_none());
        let bad = serde_json::json!({ "type": "video", "videoId": "dQw4w9WgXcQ!!" });
        assert!(parse_search_id(&bad).is_none());
        assert!(parse_search_id(&serde_json::json!({})).is_none());
    }

    #[test]
    fn as_u64_reads_int_or_string() {
        assert_eq!(as_u64(&serde_json::json!(140)), Some(140));
        assert_eq!(as_u64(&serde_json::json!("128000")), Some(128000));
        assert_eq!(as_u64(&serde_json::json!("  251 ")), Some(251));
        assert_eq!(as_u64(&serde_json::json!("abc")), None);
        assert_eq!(as_u64(&serde_json::json!(-3)), None);
        assert_eq!(as_u64(&serde_json::json!(null)), None);
    }

    fn adapt(url: &str, itag: i64, bitrate: u64) -> serde_json::Value {
        serde_json::json!({ "url": url, "itag": itag, "bitrate": bitrate, "type": "audio/webm; codecs=\"opus\"" })
    }

    fn payload(formats: serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "videoId": "dQw4w9WgXcQ", "lengthSeconds": 224, "adaptiveFormats": formats })
    }

    #[test]
    fn pick_prefers_opus_itag_over_bitrate() {
        // 251 @146k beats a higher-bitrate non-opus entry.
        let v = payload(serde_json::json!([
            { "url": "https://i/x140", "itag": 140, "bitrate": 300000, "type": "audio/mp4" },
            adapt("https://i/x251", 251, 146_555),
        ]));
        let p = pick_proxied_audio(&v, None).unwrap();
        assert_eq!(p.url, "https://i/x251");
        assert_eq!(p.bitrate, 146_555);
    }

    #[test]
    fn pick_ranks_251_over_250_over_249() {
        let v = payload(serde_json::json!([
            adapt("https://i/x249", 249, 54_402),
            adapt("https://i/x250", 250, 70_000),
            adapt("https://i/x251", 251, 146_555),
        ]));
        assert_eq!(pick_proxied_audio(&v, None).unwrap().url, "https://i/x251");
    }

    #[test]
    fn pick_honors_cap_then_degrades() {
        let v = payload(serde_json::json!([
            adapt("https://i/x251", 251, 146_555),
            adapt("https://i/x249", 249, 54_402),
        ]));
        // Low cap: 249 fits, 251 doesn't.
        assert_eq!(
            pick_proxied_audio(&v, Some(Quality::Low)).unwrap().url,
            "https://i/x249"
        );
        // Nothing fits: honest degrade to best, not failure.
        let v2 = payload(serde_json::json!([adapt("https://i/x251", 251, 146_555)]));
        assert_eq!(
            pick_proxied_audio(&v2, Some(Quality::Low)).unwrap().url,
            "https://i/x251"
        );
    }

    #[test]
    fn pick_skips_gir_and_empty_urls() {
        let v = payload(serde_json::json!([
            { "url": "https://rr1.googlevideo.com/x?gir=yes", "itag": 251, "bitrate": 146555, "type": "audio/webm" },
            { "url": "", "itag": 251, "bitrate": 146555, "type": "audio/webm" },
            adapt("https://i/x250", 250, 70_000),
        ]));
        assert_eq!(
            pick_proxied_audio(&v, None).unwrap().url,
            "https://i/x250"
        );
    }

    #[test]
    fn pick_rejects_empty_and_non_audio() {
        assert!(pick_proxied_audio(&payload(serde_json::json!([])), None).is_none());
        let v = payload(serde_json::json!([
            { "url": "https://i/x18", "itag": 18, "bitrate": 143886, "type": "video/mp4" },
        ]));
        assert!(pick_proxied_audio(&v, None).is_none());
        assert!(pick_proxied_audio(&serde_json::json!({}), None).is_none());
    }

    #[test]
    fn pick_tolerates_missing_itag_and_string_bitrate() {
        let v = payload(serde_json::json!([
            { "url": "https://i/x", "bitrate": "128000", "type": "audio/mp4" },
        ]));
        let p = pick_proxied_audio(&v, None).unwrap();
        assert_eq!(p.bitrate, 128000);
    }

    #[test]
    fn parse_instances_json_additive_api_only() {
        let v = serde_json::json!([
            ["good.test", { "api": true }],
            ["noapi.test", { "api": false }],
            ["https://keepscheme.test", { "api": true }],
            ["bad.test", {}],
            ["", { "api": true }],
            "garbage",
            ["short"],
        ]);
        assert_eq!(
            parse_instances_json(&v),
            vec![
                "https://good.test".to_string(),
                "https://keepscheme.test".to_string()
            ]
        );
        assert!(parse_instances_json(&serde_json::json!({})).is_empty());
        assert!(parse_instances_json(&serde_json::json!([])).is_empty());
    }

    #[test]
    fn pick_accepts_proxied_gir_urls() {
        // Regression: proxied instance URLs forward the upstream query
        // string INCLUDING gir=yes (verified live on f5.si). The throttle
        // is enforced per googlevideo host, never on the proxy — so these
        // must be picked, not filtered. Real response shape.
        let v = serde_json::json!({ "videoId": "wQGSX520OR0", "lengthSeconds": 211,
            "adaptiveFormats": [
                { "url": "https://invidious.f5.si/videoplayback?expire=1&gir=yes&itag=140", "itag": 140, "bitrate": 130516, "type": "audio/mp4; codecs=\"mp4a.40.2\"" },
                { "url": "https://invidious.f5.si/videoplayback?expire=1&gir=yes&itag=249", "itag": 249, "bitrate": 57048, "type": "audio/webm; codecs=\"opus\"" },
                { "url": "https://invidious.f5.si/videoplayback?expire=1&gir=yes&itag=251", "itag": 251, "bitrate": 148659, "type": "audio/webm; codecs=\"opus\"" },
            ] });
        let pick = pick_proxied_audio(&v, None).unwrap();
        assert!(pick.url.contains("itag=251"));
        assert_eq!(pick.bitrate, 148659);
    }

    #[test]
    fn pick_still_rejects_raw_googlevideo_gir() {
        // A misconfigured instance returning raw googlevideo links stays
        // excluded (same poison as direct InnerTube URLs).
        let v = serde_json::json!({ "videoId": "x", "lengthSeconds": 211,
            "adaptiveFormats": [
                { "url": "https://rr1.googlevideo.com/x?expire=1&gir=yes", "itag": 251, "bitrate": 148659, "type": "audio/webm" },
            ] });
        assert!(pick_proxied_audio(&v, None).is_none());
    }
}
