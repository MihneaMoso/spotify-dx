//! Piped provider: same YouTube catalog through independent backends.
//!
//! Piped instances proxy YouTube search + extraction server-side (no key),
//! so this rescues what the direct InnerTube path cannot: region-blocked
//! videos, signature-ciphered streams, and search misses from a different
//! region's index. It sits AFTER `youtube` in the chain — the self-contained
//! direct path stays first; Piped only runs when it fails, costing nothing
//! otherwise.
//!
//! Flow per healthy instance, in order:
//! 1. `GET {api}/search?q={artist} {title}&filter=videos` → ranked items.
//! 2. First duration-acceptable item wins (shared `duration_accepts` rule).
//! 3. `GET {api}/streams/{videoId}` → best non-video audio stream.
//!
//! Instance health is tracked in-process: 2 consecutive transport failures
//! cool an instance for 5 minutes; `is_available` is false only when every
//! instance is cooling. A short client timeout bounds dead hosts.

#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

use async_trait::async_trait;

use super::common::{urlencode, InstancePool};
use super::youtube;
use crate::streaming::provider::{Provider, Resolution, TrackQuery};

/// Pinned Piped API hosts. The docs prescribe dynamic instance discovery;
/// absent a stable machine-readable list, pinned hosts + health gating give
/// the same robustness with zero parsing fragility (Phase B scope).
/// Shared with the YouTube cipher-recovery path (single source).
pub(crate) const INSTANCES: &[&str] = &[
    "https://pipedapi.kavin.rocks",
    "https://pipedapi.adminforge.de",
    "https://pipedapi.reallyaweso.me",
];

/// Per-request budget: dead hosts fail fast instead of stalling the chain.
#[cfg(not(target_arch = "wasm32"))]
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);

pub struct PipedProvider {
    client: reqwest::Client,
    pool: InstancePool,
}

impl Default for PipedProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl PipedProvider {
    pub fn new() -> Self {
        Self {
            client: {
                let builder = reqwest::Client::builder();
                #[cfg(not(target_arch = "wasm32"))]
                let builder = builder.timeout(REQUEST_TIMEOUT);
                builder.build().unwrap_or_default()
            },
            pool: InstancePool::new(),
        }
    }

    /// Healthy instances in pinned order (cooling ones skipped).
    fn healthy(&self) -> Vec<String> {
        self.pool.healthy(INSTANCES)
    }

    /// Search one instance: transport failure vs content gap distinguished
    /// so dead hosts cool down while live-but-empty hosts don't get blamed.
    async fn search_one(&self, api: &str, query: &TrackQuery) -> SearchOutcome {
        let url = format!(
            "{api}/search?q={}+{}&filter=videos",
            urlencode(&query.artist),
            urlencode(&query.title)
        );
        let resp = match self.client.get(&url).send().await {
            Ok(r) => r,
            Err(_) => return SearchOutcome::TransportFail,
        };
        let val: serde_json::Value = match resp.json().await {
            Ok(v) => v,
            Err(_) => return SearchOutcome::TransportFail,
        };
        let items = match val.get("items").and_then(|i| i.as_array()) {
            Some(i) => i,
            None => return SearchOutcome::TransportFail,
        };
        for item in items {
            let id = match item
                .get("url")
                .and_then(|u| u.as_str())
                .and_then(video_id_from_url)
            {
                Some(id) => id,
                None => continue,
            };
            // Piped reports duration in seconds; -1/negative = live/upcoming.
            match item.get("duration").and_then(|d| d.as_i64()) {
                Some(secs) if secs < 0 => continue,
                Some(secs) if !youtube::duration_accepts(query.duration_ms, Some(secs as u64)) => {
                    continue;
                }
                _ => return SearchOutcome::Hit(id),
            }
        }
        SearchOutcome::Miss
    }

    /// Streams lookup for a video ID across healthy instances.
    async fn streams_lookup(&self, video_id: &str) -> Option<(String, u64, String)> {
        for api in self.healthy() {
            let url = format!("{api}/streams/{video_id}");
            let resp = match self.client.get(&url).send().await {
                Ok(r) => r,
                Err(_) => {
                    self.pool.note_failure(&api);
                    continue;
                }
            };
            let val: serde_json::Value = match resp.json().await {
                Ok(v) => v,
                Err(_) => {
                    self.pool.note_failure(&api);
                    continue;
                }
            };
            match youtube::pick_piped_audio(&val) {
                Some(pick) => {
                    self.pool.note_success(&api);
                    return Some(pick);
                }
                // Answered but content-less: instance is alive; stop here
                // (a gap, not an outage — don't burn the next host).
                None => return None,
            }
        }
        None
    }
}

/// Search outcome per instance: content gaps don't blame the host.
enum SearchOutcome {
    Hit(String),
    /// Live host, nothing acceptable (region variance — try next).
    Miss,
    /// Dead/unparseable host (cools down).
    TransportFail,
}

/// Extract a video ID from a Piped item URL (`/watch?v=...`).
/// Pure (unit-tested).
fn video_id_from_url(url: &str) -> Option<String> {
    url.split('?').nth(1)?.split('&').find_map(|pair| {
        let mut kv = pair.splitn(2, '=');
        if kv.next()? == "v" {
            kv.next().map(|v| v.to_string())
        } else {
            None
        }
    })
}

#[async_trait(?Send)]
impl Provider for PipedProvider {
    fn name(&self) -> &'static str {
        "piped"
    }

    fn is_available(&self) -> bool {
        !self.healthy().is_empty()
    }

    async fn resolve(&self, query: &TrackQuery) -> Resolution {
        // Search across healthy instances; the first acceptable hit wins.
        // Region variance is the point — a miss on one instance retries the
        // next rather than failing outright.
        let mut video_id: Option<String> = None;
        let mut searched_any = false;
        for api in self.healthy() {
            searched_any = true;
            match self.search_one(&api, query).await {
                SearchOutcome::Hit(id) => {
                    self.pool.note_success(&api);
                    video_id = Some(id);
                    break;
                }
                SearchOutcome::Miss => {
                    // Live host, region gap — try the next instance.
                }
                SearchOutcome::TransportFail => {
                    self.pool.note_failure(&api);
                }
            }
        }
        let video_id = match video_id {
            Some(id) => id,
            None if searched_any => {
                return Resolution::NotFound;
            }
            None => {
                return Resolution::Error("no healthy Piped instance".into());
            }
        };
        match self.streams_lookup(&video_id).await {
            Some((url, bitrate, mime)) => Resolution::Success {
                url,
                format: youtube::format_for_mime(&mime),
                quality: youtube::quality_for_bitrate(bitrate),
            },
            None => Resolution::Error(format!("Piped has no audio for {video_id}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_id_from_url_parses_watch_links() {
        assert_eq!(
            video_id_from_url("/watch?v=dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ".to_string())
        );
        assert_eq!(
            video_id_from_url("/watch?v=abc123&list=xyz"),
            Some("abc123".to_string())
        );
        assert_eq!(video_id_from_url("/channel/UC123"), None);
        assert_eq!(video_id_from_url(""), None);
    }

    #[test]
    fn search_response_shape() {
        // Documents the Piped /search contract this provider relies on.
        let v = serde_json::json!({ "items": [
            { "url": "/watch?v=vid1", "title": "Song", "duration": 224 },
            { "url": "/watch?v=vid2", "title": "Live", "duration": -1 }
        ] });
        let items = v["items"].as_array().unwrap();
        assert_eq!(
            video_id_from_url(items[0]["url"].as_str().unwrap()),
            Some("vid1".to_string())
        );
        assert_eq!(items[1]["duration"].as_i64(), Some(-1));
    }
}
