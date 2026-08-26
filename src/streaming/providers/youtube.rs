//! YouTube provider: InnerTube audio-only fallback.
//!
//! Uses YouTube's public InnerTube API to extract a streamable audio URL.
//! This is the "always available" fallback — lower quality but very reliable.
//!
//! Resolution flow:
//! 1. Odesli gives us a YouTube URL → extract the video ID.
//! 2. Alternatively, search YouTube by title + artist.
//! 3. Use InnerTube to get the stream URL for the best audio-only format.

use std::time::Duration;

use async_trait::async_trait;

use crate::streaming::odesli;
use crate::streaming::provider::{AudioFormat, Provider, Quality, Resolution, TrackQuery};

/// InnerTube API endpoint.
const INNERTUBE_URL: &str = "https://www.youtube.com/youtubei/v1/player";

/// InnerTube client config for web (unauthenticated).
const CLIENT_VERSION: &str = "2.20240101.00.00";

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
                .build()
                .unwrap_or_default(),
        }
    }

    /// Search YouTube for a track and return the video ID.
    async fn search_video_id(&self, title: &str, artist: &str) -> Option<String> {
        let query = format!("{artist} {title} audio");
        // Use InnerTube search endpoint instead of scraping.
        let body = serde_json::json!({
            "context": {
                "client": {
                    "clientName": "WEB",
                    "clientVersion": CLIENT_VERSION,
                }
            },
            "query": query,
        });
        let search_api = "https://www.youtube.com/youtubei/v1/search";
        let resp = self
            .client
            .post(search_api)
            .json(&body)
            .send()
            .await
            .ok()?;
        let val: serde_json::Value = resp.json().await.ok()?;
        // Extract first video ID from the response.
        let contents = val
            .get("contents")?
            .get("twoColumnSearchResultsRenderer")?
            .get("primaryContents")?
            .get("sectionListRenderer")?
            .get("contents")?
            .as_array()?
            .first()?
            .get("itemSectionRenderer")?
            .get("contents")?
            .as_array()?
            .first()?
            .get("videoRenderer")?
            .get("videoId")?
            .as_str()?
            .to_string();
        Some(contents)
    }

    /// Get a streamable audio URL for a YouTube video ID via InnerTube.
    async fn get_stream_url(&self, video_id: &str) -> Resolution {
        let body = serde_json::json!({
            "context": {
                "client": {
                    "clientName": "ANDROID",
                    "clientVersion": CLIENT_VERSION,
                }
            },
            "videoId": video_id,
        });
        let resp = match self
            .client
            .post(INNERTUBE_URL)
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
        // Find the best audio-only stream.
        let formats = val
            .get("streamingData")
            .and_then(|s| s.get("adaptiveFormats"))
            .and_then(|f| f.as_array());
        let formats = match formats {
            Some(f) => f,
            None => return Resolution::Error("no streaming formats found".into()),
        };
        // Prefer audio-only formats (mimeType starts with "audio/").
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
        Resolution::Error("no audio stream found in adaptive formats".into())
    }
}

#[async_trait]
impl Provider for YoutubeProvider {
    fn name(&self) -> &'static str {
        "youtube"
    }

    async fn resolve(&self, query: &TrackQuery) -> Resolution {
        // Step 1: Try Odesli for a YouTube URL.
        let video_id = if let Some(odesli_ids) = odesli::resolve(&query.spotify_id).await {
            if let Some(ref yt_url) = odesli_ids.youtube_url {
                odesli::extract_id_from_url(yt_url)
            } else {
                None
            }
        } else {
            None
        };
        // Step 2: If no Odesli mapping, search YouTube.
        let video_id = match video_id {
            Some(id) => id,
            None => match self.search_video_id(&query.title, &query.artist).await {
                Some(id) => id,
                None => return Resolution::NotFound,
            },
        };
        // Step 3: Get the stream URL.
        self.get_stream_url(&video_id).await
    }
}
