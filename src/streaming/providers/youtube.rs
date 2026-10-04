//! YouTube provider: InnerTube audio-only fallback.
//!
//! Uses YouTube's private InnerTube API to search for a track by title +
//! artist and extract a streamable audio URL. This is the "always available"
//! fallback — lower quality but very reliable.
//!
//! Resolution flow:
//! 1. Search YouTube via InnerTube (ANDROID client) with ordered query
//!    variants (`artist title audio` → `title artist` → `title artist topic,
//!    catching auto-generated/Topic uploads the first query misses).
//! 2. Collect candidate video IDs in rank order (deduplicated).
//! 3. For each candidate: duration-match against the track (±15s; candidates
//!    without a duration are accepted — legacy behavior), then the InnerTube
//!    player endpoint for the progressive muxed stream URL.
//! 4. Ciphered URLs (no direct `url`) recover through Piped `/streams` for
//!    the same video ID instead of failing.
//!
//! The ANDROID client returns direct (non-signature-encrypted) stream URLs and
//! does not require a JS footprint or Odesli mapping, so it is fully
//! self-contained.
//!
//! We deliberately prefer the progressive muxed format over the audio-only
//! adaptive formats — and exclude `gir=yes` URLs from candidacy entirely:
//! they enforce a per-(IP, content) transfer budget (~0.5–1MB, then hard
//! 403s with no refill, measured live) plus a sequential-from-zero
//! frontier, so serving one freezes the proxy prefix and skips mid-track.
//! The muxed URL serves the entire file with a plain GET.

#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

use async_trait::async_trait;

use super::common::{fanout_queries, title_matches, Fetch};
use crate::streaming::provider::{AudioFormat, Provider, Quality, Resolution, TrackQuery};

/// InnerTube API endpoints.
const INNERTUBE_SEARCH: &str = "https://www.youtube.com/youtubei/v1/search";
const INNERTUBE_PLAYER: &str = "https://www.youtube.com/youtubei/v1/player";

/// Public InnerTube API key for the ANDROID client.
const INNERTUBE_API_KEY: &str = "AIzaSyA8eiZmM1FaDVjRy-df2KTyQ_vz_yYM39w";

/// InnerTube ANDROID client context. The `osName`/`osVersion` fields and a
/// current `clientVersion` are required — stale versions are rejected with a
/// 400 `FAILED_PRECONDITION`.
const CLIENT_VERSION: &str = "20.10.38";
const USER_AGENT: &str = "com.google.android.youtube/20.10.38 (Linux; U; Android 11) gzip";

/// Max search candidates examined per resolve (bounded: each is cheap, but a
/// dead network shouldn't multiply the 15s client timeout unboundedly).
const MAX_CANDIDATES: usize = 6;
/// Max search result entries scanned per query variant.
const MAX_RESULTS_PER_QUERY: usize = 5;
/// Duration mismatch tolerance: rejects wrong uploads (mixes, hour-long
/// compilations) without dropping legit radio/remix/extended versions
/// (±30s admits version drift; the precision instrument is textual
/// matching, not this band). Shared by YouTube-direct, Qobuz, and
/// Invidious gates.
const DURATION_TOLERANCE_MS: u64 = 30_000;

pub struct YoutubeProvider {
    client: reqwest::Client,
}

impl Default for YoutubeProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl YoutubeProvider {
    pub fn new() -> Self {
        Self {
            client: {
                let builder = reqwest::Client::builder().user_agent(USER_AGENT);
                #[cfg(not(target_arch = "wasm32"))]
                let builder = builder.timeout(Duration::from_secs(15));
                builder.build().unwrap_or_default()
            },
        }
    }

    /// InnerTube ANDROID client context shared by search and player calls.
    fn innertube_context() -> serde_json::Value {
        serde_json::json!({
            "client": {
                "clientName": "ANDROID",
                "clientVersion": CLIENT_VERSION,
                "androidSdkVersion": 30,
                "userAgent": USER_AGENT,
                "osName": "Android",
                "osVersion": "11",
                "hl": "en",
                "gl": "US",
            }
        })
    }

    /// Search queries in rank order. The first is the legacy query (unchanged
    /// behavior for the common case); later variants only run when earlier
    /// ones yield nothing, so the happy path still costs one search call.
    /// Pure (unit-tested).
    fn search_queries(title: &str, artist: &str) -> [String; 3] {
        [
            format!("{artist} {title} audio"),
            format!("{title} {artist}"),
            format!("{title} {artist} topic"),
        ]
    }

    /// One search call; returns ranked `(video_id, duration_secs)` candidates.
    /// Duration comes from `lengthText` when the renderer carries it.
    async fn search_candidates(&self, query: &str) -> Vec<(String, Option<u64>)> {
        let mut out = Vec::new();
        let body = serde_json::json!({
            "context": Self::innertube_context(),
            "query": query,
        });
        let resp = match self
            .client
            .post(INNERTUBE_SEARCH)
            .query(&[("key", INNERTUBE_API_KEY)])
            .json(&body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(_) => return out,
        };
        let val: serde_json::Value = match resp.json().await {
            Ok(v) => v,
            Err(_) => return out,
        };
        // The ANDROID client returns a top-level sectionListRenderer whose
        // items use compactVideoRenderer (fall back to videoRenderer).
        let sections = match val
            .get("contents")
            .and_then(|c| c.get("sectionListRenderer"))
            .and_then(|s| s.get("contents"))
            .and_then(|c| c.as_array())
        {
            Some(s) => s,
            None => return out,
        };
        for section in sections {
            let items = match section
                .get("itemSectionRenderer")
                .and_then(|r| r.get("contents"))
                .and_then(|c| c.as_array())
            {
                Some(i) => i,
                None => continue,
            };
            for item in items {
                if out.len() >= MAX_RESULTS_PER_QUERY {
                    break;
                }
                let video = item
                    .get("compactVideoRenderer")
                    .or_else(|| item.get("videoRenderer"));
                let id = video
                    .and_then(|v| v.get("videoId"))
                    .and_then(|i| i.as_str());
                if let Some(id) = id {
                    let duration = video.and_then(parse_length_secs);
                    out.push((id.to_string(), duration));
                }
            }
        }
        out
    }
}

/// Parse `lengthText` (`simpleText` "3:44" / "1:02:03") to seconds.
/// Pure (unit-tested).
fn parse_length_secs(video: &serde_json::Value) -> Option<u64> {
    let text = length_text(video)?;
    let mut secs = 0u64;
    let parts: Vec<&str> = text.split(':').collect();
    if parts.len() > 3 || parts.is_empty() {
        return None;
    }
    for p in parts {
        secs = secs.checked_mul(60)?.checked_add(p.parse::<u64>().ok()?)?;
    }
    Some(secs)
}

/// Length text across InnerTube shapes: legacy `simpleText` and the
/// current `runs[0].text` (the ANDROID search dropped `simpleText`,
/// which silently zeroed every candidate duration until this read both).
/// Pure (unit-tested).
fn length_text(video: &serde_json::Value) -> Option<String> {
    let l = video.get("lengthText")?;
    if let Some(s) = l.get("simpleText").and_then(|s| s.as_str()) {
        return Some(s.to_string());
    }
    l.get("runs")?
        .as_array()?
        .first()?
        .get("text")?
        .as_str()
        .map(|s| s.to_string())
}

/// Internal stream outcome: distinguishes "try the next video" (content
/// problems) from "abort the chain" (transport problems — retrying more
/// videos on a dead network only multiplies the timeout).
enum StreamOutcome {
    Playable {
        url: String,
        format: AudioFormat,
        quality: Quality,
    },
    /// Content unusable (region-block, no formats, cipher unrecoverable…).
    NextCandidate(String),
    /// Transport failure (request/parse). Abort, don't burn more timeouts.
    Abort(String),
}

pub(crate) fn quality_for_bitrate(bitrate: u64) -> Quality {
    if bitrate >= 256_000 {
        Quality::High
    } else if bitrate >= 128_000 {
        Quality::Normal
    } else {
        Quality::Low
    }
}

pub(crate) fn format_for_mime(mime: &str) -> AudioFormat {
    if mime.contains("opus") {
        AudioFormat::Opus
    } else if mime.contains("mp4") || mime.contains("aac") {
        AudioFormat::Aac
    } else {
        AudioFormat::Unknown
    }
}

/// Whether a stream URL is throttle-poisoned: `gir=yes` on a GOOGLEVIDEO
/// host enforces a per-(client-IP, content) transfer budget (~0.5–1MB,
/// then hard 403s with no refill — measured live, including across fresh
/// URLs) plus a sequential-from-zero frontier. Serving one freezes the
/// proxy prefix and skips mid-track. Such URLs are never playable through
/// us — excluded from candidacy so the chain can try other providers.
/// The host check is load-bearing: proxied (Invidious instance) URLs
/// forward the upstream query string INCLUDING `gir=yes`, which is
/// harmless there (enforcement happens on googlevideo hosts, never on
/// the proxy) — excluding those broke all Invidious picks. Matched as an
/// exact query param. Pure (unit-tested).
pub(crate) fn is_throttled_url(url: &str) -> bool {
    let host = url.split("://").nth(1).unwrap_or("").split('/').next().unwrap_or("");
    if !host.ends_with("googlevideo.com") {
        return false;
    }
    let q = url.split('?').nth(1).unwrap_or("").split('#').next().unwrap_or("");
    if q.is_empty() {
        return false;
    }
    let params: Vec<&str> = q.split('&').collect();
    params.contains(&"gir=yes") && !params.contains(&"ratebypass=yes")
}

/// Adaptive selection outcome (preserves the cipher-recovery path:
/// a best entry with an empty URL still routes to `piped_recovery`).
#[derive(Debug)]
enum AdaptivePick {
    Playable { url: String, format: AudioFormat, quality: Quality },
    Encrypted,
    None,
}

/// Muxed pick over `streamingData.formats`: smallest video container
/// whose URL is present, unthrottled, and (in strict mode) within the
/// ceiling. Non-strict mode drops only the ceiling requirement — the
/// honest-degrade fallback when adaptive has nothing usable (a clean
/// over-cap muxed that plays beats skipping a playable video). Pure.
fn select_muxed(
    data: &serde_json::Value,
    capped: bool,
    ceiling: u64,
    strict_cap: bool,
) -> Option<(String, AudioFormat, Quality)> {
    let muxed = data
        .get("formats")?
        .as_array()?
        .iter()
        .filter(|f| {
            f.get("mimeType")
                .and_then(|m| m.as_str())
                .map(|m| m.starts_with("video/"))
                .unwrap_or(false)
        })
        // Smallest mux, not largest: we play audio, and the old
        // max-bitrate pick downloaded the biggest video container
        // whenever higher-resolution muxes existed — multiples
        // of the bytes (and disk) for zero audible benefit.
        // Missing bitrates sort last (unknown size, not free).
        .min_by_key(|f| {
            f.get("bitrate")
                .and_then(|b| b.as_u64())
                .unwrap_or(u64::MAX)
        })?;
    let url = muxed
        .get("url")
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string();
    if url.is_empty() || is_throttled_url(&url) {
        return None;
    }
    let bitrate = muxed.get("bitrate").and_then(|b| b.as_u64()).unwrap_or(0);
    // Strict: an over-ceiling (or unknown-bitrate) mux yields to the
    // adaptive pick below, which has real variants to choose from.
    // Non-strict (fallback only): any clean muxed plays, honestly labeled.
    // Uncapped: today's pick, untouched.
    if !strict_cap || !capped || (bitrate > 0 && bitrate <= ceiling) {
        Some((
            url,
            AudioFormat::Aac,
            quality_for_bitrate(bitrate),
        ))
    } else {
        None
    }
}

/// Adaptive pick over `streamingData.adaptiveFormats`: best audio URL
/// that is present-or-encrypted, unthrottled, and (preferably) within
/// the ceiling. Throttled entries are excluded, not degraded to — a cap
/// must never promote an unplayable URL. Pure.
fn select_adaptive(
    data: &serde_json::Value,
    capped: bool,
    ceiling: u64,
) -> AdaptivePick {
    let formats = match data.get("adaptiveFormats").and_then(|f| f.as_array()) {
        Some(f) => f,
        None => return AdaptivePick::None,
    };
    // Capped: best variant within the ceiling; uncapped: best overall
    // (today's max_by_key). An empty within-cap set falls through to
    // the degraded pick below — a cap never fails the candidate.
    // Entries with missing URLs stay eligible (empty routes to cipher
    // recovery, as today); throttled URLs never do.
    let usable = |f: &&serde_json::Value| {
        f.get("mimeType")
            .and_then(|m| m.as_str())
            .map(|m| m.starts_with("audio/"))
            .unwrap_or(false)
            && f.get("url")
                .and_then(|u| u.as_str())
                .map(|u| !is_throttled_url(u))
                .unwrap_or(true)
    };
    let in_cap: Vec<_> = formats
        .iter()
        .filter(usable)
        .filter(|f| {
            !capped
                || f.get("bitrate").and_then(|b| b.as_u64()).is_some_and(|b| b <= ceiling)
        })
        .collect();
    let best_audio = if capped && in_cap.is_empty() {
        formats
            .iter()
            .filter(usable)
            .max_by_key(|f| f.get("bitrate").and_then(|b| b.as_u64()).unwrap_or(0))
    } else {
        in_cap
            .into_iter()
            .max_by_key(|f| f.get("bitrate").and_then(|b| b.as_u64()).unwrap_or(0))
    };
    match best_audio {
        None => AdaptivePick::None,
        Some(fmt) => {
            let url = fmt
                .get("url")
                .and_then(|u| u.as_str())
                .unwrap_or("")
                .to_string();
            if url.is_empty() {
                return AdaptivePick::Encrypted;
            }
            let mime = fmt.get("mimeType").and_then(|m| m.as_str()).unwrap_or("");
            let bitrate = fmt.get("bitrate").and_then(|b| b.as_u64()).unwrap_or(0);
            AdaptivePick::Playable {
                url,
                format: format_for_mime(mime),
                quality: quality_for_bitrate(bitrate),
            }
        }
    }
}

/// Authoritative title check over a player response: the candidate's
/// `videoDetails` must name the track (see `common::title_matches`).
/// Unparseable details can't verify → false (biased strict: a false
/// reject tries the next candidate). Pure (unit-tested).
fn details_match(val: &serde_json::Value, title: &str, artist: &str) -> bool {
    let vtitle = val
        .get("videoDetails")
        .and_then(|d| d.get("title"))
        .and_then(|t| t.as_str())
        .unwrap_or("");
    if vtitle.is_empty() {
        return false;
    }
    let vauthor = val
        .get("videoDetails")
        .and_then(|d| d.get("author"))
        .and_then(|a| a.as_str())
        .unwrap_or("");
    title_matches(title, artist, vtitle, vauthor)
}

/// Best non-video audio stream from a Piped `/streams` payload as
/// `(url, bitrate, mime)`. Throttled (`gir=yes`) URLs are excluded like
/// direct InnerTube ones — Piped sometimes returns raw googlevideo URLs
/// carrying the same poison. Pure (unit-tested).
pub(crate) fn pick_piped_audio(val: &serde_json::Value) -> Option<(String, u64, String)> {
    val.get("audioStreams")?
        .as_array()?
        .iter()
        .filter(|s| {
            !s.get("videoOnly")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        })
        .filter_map(|s| {
            let url = s.get("url")?.as_str()?.to_string();
            if url.is_empty() || is_throttled_url(&url) {
                return None;
            }
            let bitrate = s.get("bitrate").and_then(|b| b.as_u64()).unwrap_or(0);
            let mime = s
                .get("mimeType")
                .and_then(|m| m.as_str())
                .unwrap_or("")
                .to_string();
            Some((url, bitrate, mime))
        })
        .max_by_key(|(_, bitrate, _)| *bitrate)
}

/// Duration acceptance: candidates carrying a duration must land within
/// tolerance of the track; duration-less candidates keep legacy accept.
/// A zero track duration means OUR metadata lacks it — accept rather than
/// reject everything (no regression for duration-less tracks).
/// Pure (unit-tested).
pub(crate) fn duration_accepts(track_ms: u64, candidate_secs: Option<u64>) -> bool {
    if track_ms == 0 {
        return true;
    }
    match candidate_secs {
        None => true,
        Some(secs) => track_ms.abs_diff(secs.saturating_mul(1000)) <= DURATION_TOLERANCE_MS,
    }
}

/// Whether a candidate carries a usable duration at all. Duration-less
/// uploads (live streams, premieres, some age-gated/mismatched entries)
/// must NOT ride the `duration_accepts` legacy-accept when OUR metadata
/// has a duration — that hole played an 8:45 video for a 3:32 track
/// (proven by file size on-device). Same rule Saavn already enforces.
/// Track-unknown durations (0) keep legacy accept. Pure (unit-tested).
pub(crate) fn has_usable_duration(track_ms: u64, candidate_secs: Option<u64>) -> bool {
    track_ms == 0 || candidate_secs.is_some()
}

impl YoutubeProvider {
    /// Get a streamable audio URL for a YouTube video ID via InnerTube.
    /// `query` carries the advisory bitrate ceiling plus the track
    /// identity for textual verification (same-language near-identical
    /// titles with clustered durations defeat duration-only gating —
    /// proven: a 186s wrong song served for a 173s track).
    async fn get_stream_url(&self, video_id: &str, query: &TrackQuery) -> StreamOutcome {
        let body = serde_json::json!({
            "context": Self::innertube_context(),
            "videoId": video_id,
            "contentCheckOk": true,
            "racyCheckOk": true,
        });
        let resp = match self
            .client
            .post(INNERTUBE_PLAYER)
            .query(&[("key", INNERTUBE_API_KEY)])
            .json(&body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => return StreamOutcome::Abort(format!("InnerTube request: {e}")),
        };
        let val: serde_json::Value = match resp.json().await {
            Ok(v) => v,
            Err(e) => return StreamOutcome::Abort(format!("InnerTube parse: {e}")),
        };
        // Check for playability status.
        let status = val
            .get("playabilityStatus")
            .and_then(|s| s.get("status"))
            .and_then(|s| s.as_str())
            .unwrap_or("unknown");
        if status != "OK" {
            let reason = val
                .get("playabilityStatus")
                .and_then(|s| s.get("reason"))
                .and_then(|r| r.as_str())
                .unwrap_or("unknown reason");
            return StreamOutcome::NextCandidate(format!(
                "InnerTube playability: {status} — {reason}"
            ));
        }
        // Textual verification: the candidate's title must name the track
        // (same-language near-identical titles with clustered durations
        // defeat duration-only gating). Biased strict — false rejects try
        // the next candidate; unparseable details can't verify either.
        if !details_match(&val, &query.title, &query.artist) {
            return StreamOutcome::NextCandidate(format!("title mismatch for {video_id}"));
        }
        let data = match val.get("streamingData") {
            Some(d) => d,
            None => {
                return StreamOutcome::NextCandidate("no streamingData found".into());
            }
        };
        // Prefer the progressive muxed format (`streamingData.formats`).
        //
        // Throttled (`gir=yes`) URLs are excluded from candidacy entirely
        // (see `is_throttled_url`): they hard-wall mid-track through any
        // proxy, so serving one guarantees a fixed-timestamp skip. The
        // progressive muxed format has no such restriction — a plain GET
        // returns the entire file. It carries an AAC audio track
        // (128kbps, the 360p muxed container), which rodio decodes fine.
        // Bitrate ceiling from the advisory cap (bands mirror
        // quality_for_bitrate so the pick and the label always agree).
        // Uncapped = u64::MAX (today's behavior, bit-for-bit).
        let ceiling: u64 = match query.max_quality {
            Some(Quality::Low) => 127_999,
            Some(Quality::Normal) => 255_999,
            _ => u64::MAX,
        };
        let capped = ceiling != u64::MAX;
        if let Some((url, format, quality)) = select_muxed(data, capped, ceiling, true) {
            return StreamOutcome::Playable {
                url,
                format,
                quality,
            };
        }
        // Fallback: best usable audio-only adaptive format
        // (`adaptiveFormats`). Throttled entries are excluded, not
        // degraded to (a cap must never promote an unplayable URL); with
        // nothing usable left the candidate is skipped so the chain can
        // try other providers — a 46s-tease-then-skip is worse than an
        // honest miss.
        if data
            .get("adaptiveFormats")
            .and_then(|f| f.as_array())
            .is_none()
        {
            return StreamOutcome::NextCandidate("no streaming formats found".into());
        }
        match select_adaptive(data, capped, ceiling) {
            AdaptivePick::Playable {
                url,
                format,
                quality,
            } => {
                return StreamOutcome::Playable {
                    url,
                    format,
                    quality,
                };
            }
            AdaptivePick::Encrypted => {
                // Signature-encrypted: recover the same video through Piped
                // instead of failing (Phase A cipher path).
                return self.piped_recovery(video_id).await;
            }
            AdaptivePick::None => {
                // Last resort before skipping the candidate: a clean
                // over-cap muxed that plays (honestly labeled) beats
                // abandoning a playable video to a strict cap.
                if let Some((url, format, quality)) =
                    select_muxed(data, capped, ceiling, false)
                {
                    return StreamOutcome::Playable {
                        url,
                        format,
                        quality,
                    };
                }
            }
        }
        StreamOutcome::NextCandidate("no audio stream found in streaming formats".into())
    }

    /// Ciphered-URL recovery: resolve the SAME video through Piped
    /// `/streams` (no key). Runs only when direct extraction fails, so it
    /// costs nothing on the happy path. Instances tried in order; transport
    /// failures move to the next instance, content failures end the attempt.
    async fn piped_recovery(&self, video_id: &str) -> StreamOutcome {
        // Hosts single-sourced from the Phase B pool (`piped::INSTANCES`)
        // so host churn lands in exactly one place.
        for api in super::piped::INSTANCES {
            let url = format!("{api}/streams/{video_id}");
            let resp = match self.client.get(&url).send().await {
                Ok(r) => r,
                Err(_) => continue,
            };
            let val: serde_json::Value = match resp.json().await {
                Ok(v) => v,
                Err(_) => continue,
            };
            if let Some(best) = pick_piped_audio(&val) {
                return StreamOutcome::Playable {
                    url: best.0,
                    format: format_for_mime(&best.2),
                    quality: quality_for_bitrate(best.1),
                };
            }
            // Instance answered but has no usable audio: don't burn the next
            // instance on what looks like a content gap, not an outage.
            return StreamOutcome::NextCandidate(format!(
                "Piped has no audio stream for {video_id}"
            ));
        }
        StreamOutcome::NextCandidate(format!(
            "stream URL requires signature decryption ({video_id})"
        ))
    }
}

#[async_trait(?Send)]
impl Provider for YoutubeProvider {
    fn name(&self) -> &'static str {
        "youtube"
    }

    async fn resolve(&self, query: &TrackQuery) -> Resolution {
        // Ranked candidates across query variants, accumulated to the cap
        // (deduplicated). Later variants fill what earlier ones left thin —
        // a junk-filled first variant must not hide better later ones —
        // and stop early once full, so the happy path still costs a single
        // search call. Odesli (song.link) is deprecated (public API now
        // returns 401) so the mapping shortcut is skipped in favor of the
        // self-contained InnerTube search.
        let queries = Self::search_queries(&query.title, &query.artist).to_vec();
        let (candidates, _aborted): (Vec<(String, Option<u64>)>, bool) = fanout_queries(
            &queries,
            MAX_CANDIDATES,
            |(id, _): &(String, Option<u64>)| id.clone(),
            |q| async { Fetch::Items(self.search_candidates(q).await) },
        )
        .await;
        if candidates.is_empty() {
            return Resolution::NotFound;
        }
        let mut last_reason = String::new();
        for (video_id, duration) in candidates {
            if !has_usable_duration(query.duration_ms, duration) {
                last_reason = format!("duration unknown for {video_id}");
                continue;
            }
            if !duration_accepts(query.duration_ms, duration) {
                last_reason = format!("duration mismatch for {video_id}");
                continue;
            }
            match self.get_stream_url(&video_id, query).await {
                StreamOutcome::Playable {
                    url,
                    format,
                    quality,
                } => {
                    return Resolution::Success {
                        url,
                        format,
                        quality,
                    };
                }
                StreamOutcome::NextCandidate(reason) => {
                    last_reason = format!("{video_id}: {reason}");
                }
                StreamOutcome::Abort(reason) => {
                    return Resolution::Error(reason);
                }
            }
        }
        if last_reason.is_empty() {
            Resolution::NotFound
        } else {
            Resolution::Error(last_reason)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_queries_rank_legacy_first() {
        let qs = YoutubeProvider::search_queries("Mercy", "Kanye West");
        assert_eq!(qs[0], "Kanye West Mercy audio");
        assert_eq!(qs[1], "Mercy Kanye West");
        assert_eq!(qs[2], "Mercy Kanye West topic");
    }

    #[test]
    fn parse_length_secs_handles_shapes() {
        let v = |s: &str| serde_json::json!({ "lengthText": { "simpleText": s } });
        assert_eq!(parse_length_secs(&v("3:44")), Some(224));
        assert_eq!(parse_length_secs(&v("1:02:03")), Some(3723));
        assert_eq!(parse_length_secs(&v("0:45")), Some(45));
        assert_eq!(parse_length_secs(&v("live")), None);
        assert!(parse_length_secs(&serde_json::json!({})).is_none());
        // Current ANDROID shape: runs[0].text (simpleText gone).
        let r = |s: &str| {
            serde_json::json!({ "lengthText": { "runs": [{ "text": s }] } })
        };
        assert_eq!(parse_length_secs(&r("3:31")), Some(211));
        assert_eq!(parse_length_secs(&r("1:02:03")), Some(3723));
        assert_eq!(parse_length_secs(&r("live")), None);
        assert!(parse_length_secs(&serde_json::json!({ "lengthText": {} })).is_none());
        assert!(parse_length_secs(&serde_json::json!({ "lengthText": { "runs": [] } })).is_none());
    }

    #[test]
    fn duration_accepts_tolerates_and_rejects() {
        // Exact + within tolerance.
        assert!(duration_accepts(224_000, Some(224)));
        assert!(duration_accepts(224_000, Some(230)));
        // 10-minute video for a 3-minute song: reject.
        assert!(!duration_accepts(224_000, Some(600)));
        // No duration carried: legacy accept (no regression).
        assert!(duration_accepts(224_000, None));
        // Zero-length track metadata never rejects a real video.
        assert!(duration_accepts(0, Some(200)));
    }

    #[test]
    fn usable_duration_required_when_track_known() {
        // Duration-less uploads are rejected when our metadata has a
        // duration (the 8:45-for-3:32 hole); unknown tracks keep accept.
        assert!(!has_usable_duration(211_764, None));
        assert!(has_usable_duration(211_764, Some(212)));
        assert!(has_usable_duration(0, None));
        assert!(has_usable_duration(0, Some(212)));
    }

    #[test]
    fn band_admits_version_drift_rejects_mixes() {
        // ±30s: the 195s remix of a 212s track passes (version drift);
        // hour-long compilations still die; textual is the precision net.
        assert!(duration_accepts(211_764, Some(195)));
        assert!(duration_accepts(211_764, Some(240)));
        assert!(!duration_accepts(211_764, Some(600)));
        assert!(!duration_accepts(211_764, Some(30)));
    }

    #[test]
    fn details_match_names_the_track() {
        let v = |title: &str, author: &str| {
            serde_json::json!({ "videoDetails": { "title": title, "author": author } })
        };
        assert!(details_match(
            &v("Jean Gaoaza - E Amarata (Official Video)", "Jean Gaoaza"),
            "E Amarata",
            "Jean Gaoaza",
        ));
        assert!(!details_match(
            &v(
                "Jean Gaoaza - Bombardierii Amărâți (Official Video)",
                "Jean Gaoaza"
            ),
            "E Amarata",
            "Jean Gaoaza",
        ));
        assert!(!details_match(
            &serde_json::json!({}),
            "E Amarata",
            "Jean Gaoaza",
        ));
        assert!(!details_match(
            &serde_json::json!({ "videoDetails": {} }),
            "E Amarata",
            "Jean Gaoaza",
        ));
    }

    #[test]
    fn pick_piped_audio_prefers_best_bitrate_url() {
        let v = serde_json::json!({ "audioStreams": [
            { "url": "", "bitrate": 999999, "mimeType": "audio/mp4", "videoOnly": false },
            { "url": "https://x/opus", "bitrate": 160000, "mimeType": "audio/webm; codecs=\"opus\"", "videoOnly": false },
            { "url": "https://x/aac", "bitrate": 128000, "mimeType": "audio/mp4", "videoOnly": false },
            { "url": "https://x/vonly", "bitrate": 999999, "mimeType": "audio/mp4", "videoOnly": true }
        ] });
        let (url, bitrate, mime) = pick_piped_audio(&v).unwrap();
        assert_eq!(url, "https://x/opus");
        assert_eq!(bitrate, 160000);
        assert!(mime.contains("opus"));
        assert!(pick_piped_audio(&serde_json::json!({})).is_none());
        assert!(pick_piped_audio(&serde_json::json!({ "audioStreams": [] })).is_none());
    }

    #[test]
    fn quality_and_format_mapping() {
        assert_eq!(quality_for_bitrate(300_000), Quality::High);
        assert_eq!(quality_for_bitrate(128_000), Quality::Normal);
        assert_eq!(quality_for_bitrate(48_000), Quality::Low);
        assert_eq!(
            format_for_mime("audio/webm; codecs=\"opus\""),
            AudioFormat::Opus
        );
        assert_eq!(format_for_mime("audio/mp4"), AudioFormat::Aac);
    }

    #[test]
    fn throttled_urls_detected_as_exact_query_param() {
        assert!(is_throttled_url(
            "https://rr1.googlevideo.com/x?expire=1&gir=yes&clen=9"
        ));
        assert!(is_throttled_url(
            "https://rr1.googlevideo.com/x?clen=9&gir=yes"
        ));
        assert!(!is_throttled_url(
            "https://rr1.googlevideo.com/x?expire=1&gir=no"
        ));
        assert!(!is_throttled_url(
            "https://rr1.googlevideo.com/x?expire=1"
        ));
        assert!(!is_throttled_url("https://host.test/gir=yes/file"));
        assert!(!is_throttled_url(""));
        assert!(!is_throttled_url("not a url"));
        // Proxied URLs forward the upstream query string INCLUDING gir —
        // harmless (enforcement is per googlevideo host): must NOT match.
        assert!(!is_throttled_url(
            "https://invidious.f5.si/videoplayback?expire=1&gir=yes&clen=9"
        ));
        assert!(!is_throttled_url(
            "https://invidious.f5.si/videoplayback?expire=1"
        ));
        // ratebypass OVERRIDES gir (measured: full multi-MB fetch, all
        // 206s — the bypass flag restores the plain-GET contract).
        assert!(!is_throttled_url(
            "https://rr1.googlevideo.com/x?expire=1&gir=yes&ratebypass=yes&clen=9"
        ));
    }

    fn stream_data(muxed: serde_json::Value, adaptive: serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "formats": muxed, "adaptiveFormats": adaptive })
    }

    fn muxed(url: &str, bitrate: u64) -> serde_json::Value {
        serde_json::json!({ "mimeType": "video/mp4", "bitrate": bitrate, "url": url })
    }

    fn adapt(url: &str, bitrate: u64) -> serde_json::Value {
        serde_json::json!({ "mimeType": "audio/webm; codecs=\"opus\"", "bitrate": bitrate, "url": url })
    }

    const GIR: &str = "https://rr1.googlevideo.com/x?expire=1&gir=yes";
    const CLEAN: &str = "https://rr1.googlevideo.com/x?expire=1";

    #[test]
    fn muxed_gir_yields_to_clean_adaptive() {
        // Throttled muxed is skipped even though muxed ranks first.
        let data = stream_data(
            serde_json::json!([muxed(GIR, 143_886)]),
            serde_json::json!([adapt(CLEAN, 146_555)]),
        );
        assert!(select_muxed(&data, false, u64::MAX, true).is_none());
        match select_adaptive(&data, false, u64::MAX) {
            AdaptivePick::Playable { url, .. } => assert_eq!(url, CLEAN),
            other => panic!("expected playable, got {other:?}"),
        }
    }

    #[test]
    fn clean_muxed_beats_gir_adaptive() {
        let data = stream_data(
            serde_json::json!([muxed(CLEAN, 143_886)]),
            serde_json::json!([adapt(GIR, 146_555)]),
        );
        let (url, _, _) = select_muxed(&data, false, u64::MAX, true).unwrap();
        assert_eq!(url, CLEAN);
    }

    #[test]
    fn all_gir_selects_nothing() {
        let data = stream_data(
            serde_json::json!([muxed(GIR, 143_886)]),
            serde_json::json!([adapt(GIR, 146_555), adapt(GIR, 54_402)]),
        );
        assert!(select_muxed(&data, false, u64::MAX, true).is_none());
        assert!(matches!(
            select_adaptive(&data, false, u64::MAX),
            AdaptivePick::None
        ));
        // Capped or not, poison stays excluded (never promoted by degrade).
        assert!(matches!(
            select_adaptive(&data, true, 127_999),
            AdaptivePick::None
        ));
    }

    #[test]
    fn encrypted_adaptive_still_routes_to_recovery() {
        let data = stream_data(
            serde_json::json!([]),
            serde_json::json!([{ "mimeType": "audio/mp4", "bitrate": 128000, "url": "" }]),
        );
        assert!(matches!(
            select_adaptive(&data, false, u64::MAX),
            AdaptivePick::Encrypted
        ));
    }

    #[test]
    fn capped_muxed_yields_to_clean_adaptive() {
        // Over-ceiling muxed yields (today's behavior), gir muxed never wins.
        let data = stream_data(
            serde_json::json!([muxed(CLEAN, 500_000)]),
            serde_json::json!([adapt(CLEAN, 130_567)]),
        );
        assert!(select_muxed(&data, true, 255_999, true).is_none());
        match select_adaptive(&data, true, 255_999) {
            AdaptivePick::Playable { url, .. } => assert_eq!(url, CLEAN),
            other => panic!("expected playable, got {other:?}"),
        }
    }

    #[test]
    fn pick_piped_audio_rejects_throttled() {
        let v = serde_json::json!({ "audioStreams": [
            { "url": "https://rr1.googlevideo.com/x?gir=yes", "bitrate": 999999, "mimeType": "audio/mp4", "videoOnly": false },
            { "url": "https://x/aac", "bitrate": 128000, "mimeType": "audio/mp4", "videoOnly": false },
        ] });
        let (url, _, _) = pick_piped_audio(&v).unwrap();
        assert_eq!(url, "https://x/aac");
    }

    #[test]
    fn over_cap_muxed_rescued_when_adaptive_all_gir() {
        // Strict cap excludes the clean muxed and gir poisons adaptive —
        // the fallback serves the muxed honestly labeled instead of
        // abandoning a playable video.
        let data = stream_data(
            serde_json::json!([muxed(CLEAN, 143_886)]),
            serde_json::json!([adapt(GIR, 54_402)]),
        );
        assert!(select_muxed(&data, true, 127_999, true).is_none());
        assert!(matches!(
            select_adaptive(&data, true, 127_999),
            AdaptivePick::None
        ));
        let (url, _, quality) = select_muxed(&data, true, 127_999, false).unwrap();
        assert_eq!(url, CLEAN);
        assert_eq!(quality, Quality::Normal);
    }





}
