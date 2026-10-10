//! Deezer 30s preview layer: keyless mainstream backfill.
//!
//! The public `api.deezer.com` search needs no key and carries the full
//! mainstream catalog (ISRC-linked, excellently ranked), but only serves
//! 30-second signed MP3 previews (`preview` field, `exp=` expiry —
//! re-fetch at play time, never cache). It sits LAST in the chain: an
//! honest labeled preview beats NOT_FOUND, and it answers in ~1s where
//! the full-track fallthrough burns tens of seconds.
//!
//! Flow: `GET /search/track?q={artist title}&limit=5` (plain query —
//! fielded `artist:"…" track:"…"` misses on punctuation like `&`, so
//! ranking + our own gates do the precision work) → first result passing
//! the shared textual + duration gates → its `preview` URL verbatim.
//! Quota errors (`error.code == 4`, ~50 req/5s/IP) arrive as HTTP 200
//! with an error body → `Cooldown`, never cached as a miss.
//!
//! Verified live Oct 2026 (search → HEAD 200 audio/mpeg on the preview).

use async_trait::async_trait;

use super::common::{duration_tight, http_client, title_matches, title_matches_fuzzy, urlencode};
use super::youtube;
use crate::streaming::provider::{AudioFormat, Provider, Quality, Resolution, TrackQuery};

const API: &str = "https://api.deezer.com";
const UA: &str = "spotify-dx/1";

pub struct DeezerProvider {
    client: reqwest::Client,
}

impl Default for DeezerProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl DeezerProvider {
    pub fn new() -> Self {
        Self {
            client: http_client(UA, 10),
        }
    }

    /// Pure pick over a decoded search body: first result passing the
    /// shared gates, or the quota signal. Unit-tested with canned payloads.
    pub(crate) fn pick(
        body: &serde_json::Value,
        title: &str,
        artist: &str,
        track_ms: u64,
    ) -> Result<Option<String>, PickError> {
        if body
            .get("error")
            .and_then(|e| e.get("code"))
            .and_then(|c| c.as_u64())
            == Some(4)
        {
            return Err(PickError::Quota);
        }
        let items = body
            .get("data")
            .and_then(|d| d.as_array())
            .cloned()
            .unwrap_or_default();
        for t in items {
            let ttitle = t.get("title").and_then(|x| x.as_str()).unwrap_or("");
            let tartist = t
                .get("artist")
                .and_then(|a| a.get("name"))
                .and_then(|x| x.as_str())
                .unwrap_or("");
            let duration = t.get("duration").and_then(|d| d.as_u64());
            // Exact gate, else drift forgiveness on tight durations only.
            let gate = if duration_tight(track_ms, duration) {
                title_matches_fuzzy(title, artist, ttitle, tartist)
            } else {
                title_matches(title, artist, ttitle, tartist)
            };
            if !gate {
                continue;
            }
            if !youtube::duration_accepts(track_ms, duration) {
                continue;
            }
            // Duration-less Deezer rows can't verify (same rule as
            // `has_usable_duration` elsewhere): without our duration the
            // check above legacy-accepts, so require the field when WE
            // know the track length.
            if track_ms != 0 && duration.is_none() {
                continue;
            }
            if let Some(url) = t.get("preview").and_then(|u| u.as_str()) {
                if !url.is_empty() {
                    return Ok(Some(url.to_string()));
                }
            }
        }
        Ok(None)
    }
}

/// Pick failure modes: quota is transient (cooldown), everything else is a
/// content verdict for the caller to map.
#[derive(Debug, PartialEq)]
pub(crate) enum PickError {
    Quota,
}

#[async_trait(?Send)]
impl Provider for DeezerProvider {
    fn name(&self) -> &'static str {
        "deezer"
    }

    /// Previews are signed minutes-out URLs: never cacheable (the 50-min
    /// stream cache would serve dead signatures). The resolver consults
    /// this before writing.
    fn cacheable(&self) -> bool {
        false
    }

    async fn resolve(&self, query: &TrackQuery) -> Resolution {
        let url = format!(
            "{API}/search/track?q={}&limit=5",
            urlencode(&format!("{} {}", query.artist, query.title))
        );
        let body: serde_json::Value = match self.client.get(&url).send().await {
            Ok(r) => match r.json().await {
                Ok(v) => v,
                Err(e) => {
                    return Resolution::Error(format!("deezer parse: {e}"));
                }
            },
            Err(e) => {
                return Resolution::Error(format!("deezer search: {e}"));
            }
        };
        match Self::pick(&body, &query.title, &query.artist, query.duration_ms) {
            Err(PickError::Quota) => Resolution::Cooldown { retry_after_secs: 60 },
            Ok(Some(preview)) => Resolution::Success {
                url: preview,
                format: AudioFormat::Mp3,
                quality: Quality::Normal,
            },
            Ok(None) => Resolution::NotFound,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body() -> serde_json::Value {
        serde_json::json!({
            "total": 2,
            "data": [
                {"title": "Konnichiwa", "artist": {"name": "Future"},
                 "duration": 154, "preview": "https://cdnt-preview/x.mp3"},
                {"title": "F&N", "artist": {"name": "Future"},
                 "duration": 189, "preview": "https://cdnt-preview/fn.mp3"},
            ]
        })
    }

    /// The F&N case end to end at pick level: short-token titles match,
    /// wrong tracks skip, exact preview wins.
    #[test]
    fn pick_exact_preview() {
        let got = DeezerProvider::pick(&body(), "F&N", "Future", 191_000);
        assert_eq!(got, Ok(Some("https://cdnt-preview/fn.mp3".to_string())));
    }

    #[test]
    fn pick_rejects_mismatches() {
        // Wrong song, right artist.
        assert_eq!(
            DeezerProvider::pick(&body(), "Mask Off", "Future", 204_000),
            Ok(None)
        );
        // Right song, absurd duration.
        assert_eq!(
            DeezerProvider::pick(&body(), "F&N", "Future", 60_000),
            Ok(None)
        );
    }

    #[test]
    fn pick_quota_maps_to_cooldown_signal() {
        let q = serde_json::json!({"error": {"code": 4, "message": "Quota limit exceeded"}});
        assert_eq!(
            DeezerProvider::pick(&q, "F&N", "Future", 191_000),
            Err(PickError::Quota)
        );
    }

    #[test]
    fn pick_empty_preview_url_skips() {
        let b = serde_json::json!({"data": [
            {"title": "F&N", "artist": {"name": "Future"},
             "duration": 189, "preview": ""},
        ]});
        assert_eq!(DeezerProvider::pick(&b, "F&N", "Future", 191_000), Ok(None));
    }
}
