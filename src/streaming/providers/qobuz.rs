//! Qobuz provider: resolves tracks via community Qobuz proxy endpoints.
//!
//! Qobuz is matched by ISRC (International Standard Recording Code) when
//! available, which gives very accurate matches. Falls back to title+artist
//! search.
//!
//! Resolution flow:
//! 1. Odesli gives us a Qobuz URL → extract the Qobuz album/track ID.
//! 2. Alternatively, search Qobuz by ISRC or title+artist.
//! 3. POST to a Qobuz proxy endpoint for the stream URL.

use std::time::Duration;

use async_trait::async_trait;

use crate::streaming::odesli;
use crate::streaming::provider::{AudioFormat, Provider, Quality, Resolution, TrackQuery};

/// Community Qobuz proxy instances.
const QOBUZ_INSTANCES: &[&str] = &[
    "https://api.qobuz.com",  // public API (limited)
];

pub struct QobuzProvider {
    client: reqwest::Client,
}

impl Default for QobuzProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl QobuzProvider {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
        }
    }

    /// Try to get a stream URL by ISRC search via the Qobuz public search.
    async fn resolve_by_isrc(&self, isrc: &str) -> Resolution {
        let search_url = format!(
            "https://api.qobuz.com/api/2.0/track/search?isrc={isrc}&limit=1"
        );
        match self.client.get(&search_url).send().await {
            Ok(resp) if resp.status().is_success() => {
                if let Ok(body) = resp.text().await {
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&body) {
                        if let Some(tracks) = val.get("tracks").and_then(|t| t.as_array()) {
                            if let Some(first) = tracks.first() {
                                if let Some(url) =
                                    first.get("stream_url").and_then(|u| u.as_str())
                                {
                                    return Resolution::Success {
                                        url: url.to_string(),
                                        format: AudioFormat::Flac,
                                        quality: Quality::Lossless,
                                    };
                                }
                            }
                        }
                    }
                }
                Resolution::NotFound
            }
            Ok(resp) if resp.status().as_u16() == 503 => Resolution::Cooldown {
                retry_after_secs: 60,
            },
            Ok(resp) => Resolution::Error(format!("Qobuz search HTTP {}", resp.status())),
            Err(e) => Resolution::Error(format!("Qobuz search: {e}")),
        }
    }

    /// Try to get a stream URL via Odesli's Qobuz link.
    async fn resolve_by_odesli(&self, spotify_id: &str) -> Resolution {
        let odesli_ids = match odesli::resolve(spotify_id).await {
            Some(ids) => ids,
            None => return Resolution::Error("odesli mapping failed".into()),
        };
        let qobuz_url = match odesli_ids.qobuz_url {
            Some(url) => url,
            None => return Resolution::NotFound,
        };

        // Try each proxy instance.
        for instance in QOBUZ_INSTANCES {
            let api_url = format!(
                "{}/api/dl/{}",
                instance.trim_end_matches('/'),
                odesli::extract_id_from_url(&qobuz_url).unwrap_or_default()
            );
            match self.client.get(&api_url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    if let Ok(body) = resp.text().await {
                        let url = extract_url_from_response(&body);
                        if !url.is_empty() {
                            return Resolution::Success {
                                url,
                                format: AudioFormat::Flac,
                                quality: Quality::Lossless,
                            };
                        }
                    }
                }
                Ok(resp) if resp.status().as_u16() == 503 => {
                    return Resolution::Cooldown {
                        retry_after_secs: 60,
                    };
                }
                _ => continue,
            }
        }
        Resolution::NotFound
    }
}

#[async_trait]
impl Provider for QobuzProvider {
    fn name(&self) -> &'static str {
        "qobuz"
    }

    fn is_available(&self) -> bool {
        // DISABLED: same root cause as TIDAL — Odesli (the Spotify→Qobuz ID
        // mapper) is sunset/401, and Qobuz's own search API requires paid
        // credentials. Skip so the resolver doesn't call the dead Odesli API.
        false
    }

    async fn resolve(&self, query: &TrackQuery) -> Resolution {
        // Prefer ISRC match (most accurate).
        if let Some(ref isrc) = query.isrc {
            match self.resolve_by_isrc(isrc).await {
                res @ Resolution::Success { .. } => return res,
                res @ Resolution::Cooldown { .. } => return res,
                _ => {} // fall through to Odesli
            }
        }
        // Fall back to Odesli mapping.
        self.resolve_by_odesli(&query.spotify_id).await
    }
}

fn extract_url_from_response(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.starts_with("http") {
        return trimmed.to_string();
    }
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if let Some(url) = val.get("url").and_then(|u| u.as_str()) {
            return url.to_string();
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_url_plain() {
        assert_eq!(
            extract_url_from_response("https://qobuz.example.com/s.flac"),
            "https://qobuz.example.com/s.flac"
        );
    }

    #[test]
    fn extract_url_json() {
        let body = r#"{"url": "https://qobuz.example.com/s.flac"}"#;
        assert_eq!(
            extract_url_from_response(body),
            "https://qobuz.example.com/s.flac"
        );
    }
}
