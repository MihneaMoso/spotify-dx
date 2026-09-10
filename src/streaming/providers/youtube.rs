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
//! adaptive formats: the adaptive URLs carry `gir=yes` and are IP-bound +
//! throttled by YouTube (sustained downloads 403 after ~1MB), whereas the
//! muxed URL serves the entire file with a plain GET.

#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

use async_trait::async_trait;

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
const USER_AGENT: &str =
    "com.google.android.youtube/20.10.38 (Linux; U; Android 11) gzip";

/// Piped API hosts for ciphered-URL recovery (no key; docs prescribe dynamic
/// instance discovery — the Phase B pool supersedes this pair).
const PIPED_APIS: &[&str] = &[
    "https://pipedapi.kavin.rocks",
    "https://pipedapi.adminforge.de",
];

/// Max search candidates examined per resolve (bounded: each is cheap, but a
/// dead network shouldn't multiply the 15s client timeout unboundedly).
const MAX_CANDIDATES: usize = 6;
/// Max search result entries scanned per query variant.
const MAX_RESULTS_PER_QUERY: usize = 5;
/// Duration mismatch tolerance: rejects wrong uploads (10-min "song" videos)
/// without dropping legit live/extended versions.
const DURATION_TOLERANCE_MS: u64 = 15_000;

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
    let text = video
        .get("lengthText")
        .and_then(|l| l.get("simpleText"))
        .and_then(|s| s.as_str())?;
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

/// Internal stream outcome: distinguishes "try the next video" (content
/// problems) from "abort the chain" (transport problems — retrying more
/// videos on a dead network only multiplies the timeout).
enum StreamOutcome {
    Playable { url: String, format: AudioFormat, quality: Quality },
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

/// Best non-video audio stream from a Piped `/streams` payload as
/// `(url, bitrate, mime)`. Pure (unit-tested).
pub(crate) fn pick_piped_audio(val: &serde_json::Value) -> Option<(String, u64, String)> {
    val.get("audioStreams")?.as_array()?.iter()
        .filter(|s| {
            !s.get("videoOnly").and_then(|v| v.as_bool()).unwrap_or(false)
        })
        .filter_map(|s| {
            let url = s.get("url")?.as_str()?.to_string();
            if url.is_empty() {
                return None;
            }
            let bitrate = s.get("bitrate").and_then(|b| b.as_u64()).unwrap_or(0);
            let mime = s.get("mimeType").and_then(|m| m.as_str()).unwrap_or("").to_string();
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

impl YoutubeProvider {
    /// Get a streamable audio URL for a YouTube video ID via InnerTube.
    async fn get_stream_url(&self, video_id: &str) -> StreamOutcome {
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
        let data = match val.get("streamingData") {
            Some(d) => d,
            None => {
                return StreamOutcome::NextCandidate("no streamingData found".into());
            }
        };
        // Prefer the progressive muxed format (`streamingData.formats`).
        //
        // The adaptive audio-only formats (`adaptiveFormats`) are served from
        // googlevideo URLs with `gir=yes`, which are IP-bound and throttled:
        // a sustained download 403s after ~1MB, so a full multi-MB song can't
        // be fetched. The progressive muxed format has no such restriction —
        // a plain GET returns the entire file. It carries an AAC audio track
        // (128kbps, the 360p muxed container), which rodio decodes fine.
        if let Some(muxed) = data.get("formats").and_then(|f| f.as_array())
            .and_then(|f| {
                f.iter()
                    .filter(|f| {
                        f.get("mimeType")
                            .and_then(|m| m.as_str())
                            .map(|m| m.starts_with("video/"))
                            .unwrap_or(false)
                    })
                    .max_by_key(|f| {
                        f.get("bitrate")
                            .and_then(|b| b.as_u64())
                            .unwrap_or(0)
                    })
            })
        {
            let url = muxed
                .get("url")
                .and_then(|u| u.as_str())
                .unwrap_or("")
                .to_string();
            if !url.is_empty() {
                let bitrate = muxed
                    .get("bitrate")
                    .and_then(|b| b.as_u64())
                    .unwrap_or(0);
                return StreamOutcome::Playable {
                    url,
                    format: AudioFormat::Aac,
                    quality: quality_for_bitrate(bitrate),
                };
            }
        }
        // Fallback: best audio-only adaptive format (`adaptiveFormats`).
        // Note: these `gir=yes` URLs are throttled (~1MB cap) on sustained
        // downloads and may fail for full-length tracks.
        let formats = match data.get("adaptiveFormats").and_then(|f| f.as_array()) {
            Some(f) => f,
            None => {
                return StreamOutcome::NextCandidate("no streaming formats found".into());
            }
        };
        let best_audio = formats
            .iter()
            .filter(|f| {
                f.get("mimeType")
                    .and_then(|m| m.as_str())
                    .map(|m| m.starts_with("audio/"))
                    .unwrap_or(false)
            })
            .max_by_key(|f| {
                f.get("bitrate")
                    .and_then(|b| b.as_u64())
                    .unwrap_or(0)
            });
        if let Some(fmt) = best_audio {
            let url = fmt
                .get("url")
                .and_then(|u| u.as_str())
                .unwrap_or("")
                .to_string();
            if url.is_empty() {
                // Signature-encrypted: recover the same video through Piped
                // instead of failing (Phase A cipher path).
                return self.piped_recovery(video_id).await;
            }
            let mime = fmt
                .get("mimeType")
                .and_then(|m| m.as_str())
                .unwrap_or("");
            let bitrate = fmt
                .get("bitrate")
                .and_then(|b| b.as_u64())
                .unwrap_or(0);
            return StreamOutcome::Playable {
                url,
                format: format_for_mime(mime),
                quality: quality_for_bitrate(bitrate),
            };
        }
        StreamOutcome::NextCandidate("no audio stream found in streaming formats".into())
    }

    /// Ciphered-URL recovery: resolve the SAME video through Piped
    /// `/streams` (no key). Runs only when direct extraction fails, so it
    /// costs nothing on the happy path. Instances tried in order; transport
    /// failures move to the next instance, content failures end the attempt.
    async fn piped_recovery(&self, video_id: &str) -> StreamOutcome {
        for api in PIPED_APIS {
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
        // Ranked candidates across query variants (deduplicated, bounded).
        // Later variants only run when earlier ones yield nothing, so the
        // happy path still costs a single search call. Odesli (song.link) is
        // deprecated (public API now returns 401) so the mapping shortcut is
        // skipped in favor of the self-contained InnerTube search.
        let mut seen = std::collections::HashSet::new();
        let mut candidates: Vec<(String, Option<u64>)> = Vec::new();
        for q in Self::search_queries(&query.title, &query.artist) {
            if candidates.len() >= MAX_CANDIDATES {
                break;
            }
            for (id, dur) in self.search_candidates(&q).await {
                if seen.insert(id.clone()) {
                    candidates.push((id, dur));
                    if candidates.len() >= MAX_CANDIDATES {
                        break;
                    }
                }
            }
            if !candidates.is_empty() {
                break;
            }
        }
        if candidates.is_empty() {
            return Resolution::NotFound;
        }
        let mut last_reason = String::new();
        for (video_id, duration) in candidates {
            if !duration_accepts(query.duration_ms, duration) {
                last_reason = format!("duration mismatch for {video_id}");
                continue;
            }
            match self.get_stream_url(&video_id).await {
                StreamOutcome::Playable { url, format, quality } => {
                    return Resolution::Success { url, format, quality };
                }
                StreamOutcome::NextCandidate(reason) => {
                    last_reason = reason;
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
        assert_eq!(parse_length_secs(&serde_json::json!({})), None);
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
        assert_eq!(format_for_mime("audio/webm; codecs=\"opus\""), AudioFormat::Opus);
        assert_eq!(format_for_mime("audio/mp4"), AudioFormat::Aac);
    }
}
