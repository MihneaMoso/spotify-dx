//! YouTube provider: InnerTube audio-only fallback.
//!
//! Uses YouTube's private InnerTube API to search for a track by title +
//! artist and extract a streamable audio URL. This is the "always available"
//! fallback — lower quality but very reliable.
//!
//! Resolution flow:
//! 1. Search YouTube via InnerTube (ANDROID client) by title + artist.
//! 2. Extract the first matching video ID (compactVideoRenderer).
//! 3. Call the InnerTube player endpoint to get the progressive muxed stream
//!    URL (the 360p mp4 containing an AAC audio track).
//!
//! The ANDROID client returns direct (non-signature-encrypted) stream URLs and
//! does not require a JS footprint or Odesli mapping, so it is fully
//! self-contained.
//!
//! We deliberately prefer the progressive muxed format over the audio-only
//! adaptive formats: the adaptive URLs carry `gir=yes` and are IP-bound +
//! throttled by YouTube (sustained downloads 403 after ~1MB), whereas the
//! muxed URL serves the entire file with a plain GET.

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
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .user_agent(USER_AGENT)
                .build()
                .unwrap_or_default(),
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

    /// Search YouTube for a track and return the first matching video ID.
    async fn search_video_id(&self, title: &str, artist: &str) -> Option<String> {
        let query = format!("{artist} {title} audio");
        let body = serde_json::json!({
            "context": Self::innertube_context(),
            "query": query,
        });
        let resp = self
            .client
            .post(INNERTUBE_SEARCH)
            .query(&[("key", INNERTUBE_API_KEY)])
            .json(&body)
            .send()
            .await
            .ok()?;
        let val: serde_json::Value = resp.json().await.ok()?;
        // The ANDROID client returns a top-level sectionListRenderer whose
        // items use compactVideoRenderer (fall back to videoRenderer).
        let sections = val
            .get("contents")?
            .get("sectionListRenderer")?
            .get("contents")?
            .as_array()?;
        for section in sections {
            let items = section.get("itemSectionRenderer")?.get("contents")?;
            for item in items.as_array()? {
                let video = item.get("compactVideoRenderer")
                    .or_else(|| item.get("videoRenderer"));
                if let Some(id) = video.and_then(|v| v.get("videoId")).and_then(|i| i.as_str()) {
                    return Some(id.to_string());
                }
            }
        }
        None
    }

    /// Get a streamable audio URL for a YouTube video ID via InnerTube.
    async fn get_stream_url(&self, video_id: &str) -> Resolution {
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
            Err(e) => return Resolution::Error(format!("InnerTube request: {e}")),
        };
        let val: serde_json::Value = match resp.json().await {
            Ok(v) => v,
            Err(e) => return Resolution::Error(format!("InnerTube parse: {e}")),
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
            return Resolution::Error(format!("InnerTube playability: {status} — {reason}"));
        }
        let data = match val.get("streamingData") {
            Some(d) => d,
            None => return Resolution::Error("no streamingData found".into()),
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
                let quality = if bitrate >= 256_000 {
                    Quality::High
                } else if bitrate >= 128_000 {
                    Quality::Normal
                } else {
                    Quality::Low
                };
                return Resolution::Success {
                    url,
                    format: AudioFormat::Aac,
                    quality,
                };
            }
        }
        // Fallback: best audio-only adaptive format (`adaptiveFormats`).
        // Note: these `gir=yes` URLs are throttled (~1MB cap) on sustained
        // downloads and may fail for full-length tracks.
        let formats = match data.get("adaptiveFormats").and_then(|f| f.as_array()) {
            Some(f) => f,
            None => return Resolution::Error("no streaming formats found".into()),
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
                // Some streams require signature decryption; skip those.
                return Resolution::Error("stream URL requires signature decryption".into());
            }
            let mime = fmt
                .get("mimeType")
                .and_then(|m| m.as_str())
                .unwrap_or("");
            let format = if mime.contains("opus") {
                AudioFormat::Opus
            } else if mime.contains("mp4") || mime.contains("aac") {
                AudioFormat::Aac
            } else {
                AudioFormat::Unknown
            };
            let bitrate = fmt
                .get("bitrate")
                .and_then(|b| b.as_u64())
                .unwrap_or(0);
            let quality = if bitrate >= 256_000 {
                Quality::High
            } else if bitrate >= 128_000 {
                Quality::Normal
            } else {
                Quality::Low
            };
            return Resolution::Success {
                url,
                format,
                quality,
            };
        }
        Resolution::Error("no audio stream found in streaming formats".into())
    }
}

#[async_trait]
impl Provider for YoutubeProvider {
    fn name(&self) -> &'static str {
        "youtube"
    }

    async fn resolve(&self, query: &TrackQuery) -> Resolution {
        // Search YouTube directly by title + artist. Odesli (song.link) is
        // deprecated (public API now returns 401) so the mapping shortcut is
        // skipped in favor of the self-contained InnerTube search.
        let video_id = match self.search_video_id(&query.title, &query.artist).await {
            Some(id) => id,
            None => return Resolution::NotFound,
        };
        // Get the stream URL.
        self.get_stream_url(&video_id).await
    }
}
