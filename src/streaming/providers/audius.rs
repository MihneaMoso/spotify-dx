//! Audius provider: open-catalog audio, no key.
//!
//! The Audius REST API serves read-only catalog + streams without
//! credentials (`app_name` identifies the client). Independent-artist
//! catalog — exactly the niche material the major-catalog providers miss.
//!
//! Flow: `GET /v1/tracks/search?query=&app_name=` → first
//! duration-acceptable, non-deleted result → `GET /v1/tracks/{id}/stream`
//! with redirects DISABLED → 302 `Location` is the signed direct URL
//! (validity ~days, far beyond the 50-minute stream cache, so caching stays
//! safe). A 200 instead means the node proxies bytes — the stream URL
//! itself then plays through it.
//!
//! Sits after `saavn`: keyless and reliable, but catalog-first for indie
//! rather than mainstream backfill.

#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;
use std::sync::Mutex;

use async_trait::async_trait;

use super::youtube;
use crate::streaming::provider::{AudioFormat, Provider, Quality, Resolution, TrackQuery};

const API: &str = "https://api.audius.co/v1";
const APP_NAME: &str = "SpotifyDX";

#[cfg(not(target_arch = "wasm32"))]
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
const COOL_AFTER_FAILURES: u32 = 3;
#[cfg(not(target_arch = "wasm32"))]
const COOLDOWN: Duration = Duration::from_secs(5 * 60);

pub struct AudiusProvider {
    client: reqwest::Client,
    /// Redirects disabled: the 302 Location IS the product.
    bare_client: reqwest::Client,
    failures: Mutex<u32>,
    #[cfg(not(target_arch = "wasm32"))]
    cooling_until: Mutex<Option<std::time::Instant>>,
}

impl Default for AudiusProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl AudiusProvider {
    pub fn new() -> Self {
        let build = || {
            let builder = reqwest::Client::builder();
            #[cfg(not(target_arch = "wasm32"))]
            let builder = builder.timeout(REQUEST_TIMEOUT);
            builder.build().unwrap_or_default()
        };
        Self {
            client: build(),
            bare_client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap_or_default(),
            failures: Mutex::new(0),
            #[cfg(not(target_arch = "wasm32"))]
            cooling_until: Mutex::new(None),
        }
    }

    fn note_success(&self) {
        if let Ok(mut f) = self.failures.lock() {
            *f = 0;
        }
    }

    fn note_failure(&self) {
        if let Ok(mut f) = self.failures.lock() {
            *f += 1;
            #[cfg(not(target_arch = "wasm32"))]
            if *f >= COOL_AFTER_FAILURES {
                if let Ok(mut c) = self.cooling_until.lock() {
                    *c = Some(std::time::Instant::now() + COOLDOWN);
                }
            }
        }
    }

    async fn resolve_inner(&self, query: &TrackQuery) -> Resolution {
        let url = format!(
            "{API}/tracks/search?query={}+{}&app_name={APP_NAME}&limit=8",
            urlencode(&query.artist),
            urlencode(&query.title)
        );
        let resp = match self.client.get(&url).send().await {
            Ok(r) => r,
            Err(_) => {
                self.note_failure();
                return Resolution::Error("audius search request failed".into());
            }
        };
        let val: serde_json::Value = match resp.json().await {
            Ok(v) => v,
            Err(_) => {
                self.note_failure();
                return Resolution::Error("audius search parse failed".into());
            }
        };
        let items = match val.get("data").and_then(|d| d.as_array()) {
            Some(i) => i,
            None => {
                self.note_failure();
                return Resolution::Error("audius search shape changed".into());
            }
        };
        let Some(id) = pick_track(items, query.duration_ms) else {
            return Resolution::NotFound;
        };
        match self.stream_url(&id).await {
            Some(url) => {
                self.note_success();
                Resolution::Success {
                    url,
                    format: AudioFormat::Unknown,
                    quality: Quality::Normal,
                }
            }
            None => Resolution::NotFound,
        }
    }

    /// Resolve the signed direct URL: 302 Location wins; a 200 means the
    /// node proxies bytes, so the stream URL itself is returned.
    async fn stream_url(&self, id: &str) -> Option<String> {
        let url = format!("{API}/tracks/{id}/stream?app_name={APP_NAME}");
        // `bare_client` never follows: inspect the raw status instead.
        let resp = self.bare_client.get(&url).send().await.ok()?;
        let status = resp.status().as_u16();
        if (300..400).contains(&status) {
            let loc = resp
                .headers()
                .get(reqwest::header::LOCATION)?
                .to_str()
                .ok()?
                .to_string();
            if loc.starts_with("http") {
                return Some(loc);
            }
            return None;
        }
        if (200..300).contains(&status) {
            return Some(url);
        }
        None
    }
}

/// First duration-acceptable, non-deleted track id, in rank order.
/// Pure (unit-tested).
fn pick_track(items: &[serde_json::Value], track_ms: u64) -> Option<String> {
    for item in items {
        if item.get("is_delete").and_then(|d| d.as_bool()).unwrap_or(false) {
            continue;
        }
        if item.get("is_unlisted").and_then(|u| u.as_bool()).unwrap_or(false) {
            continue;
        }
        let id = match item.get("id").and_then(|i| i.as_str()) {
            Some(id) if !id.is_empty() => id,
            _ => continue,
        };
        let duration = item.get("duration").and_then(|d| d.as_u64());
        if !youtube::duration_accepts(track_ms, duration) {
            continue;
        }
        return Some(id.to_string());
    }
    None
}
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else if b == b' ' {
            out.push('+');
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[async_trait(?Send)]
impl Provider for AudiusProvider {
    fn name(&self) -> &'static str {
        "audius"
    }

    fn is_available(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let cooling = self.cooling_until.lock().unwrap_or_else(|e| e.into_inner());
            if cooling.map(|u| std::time::Instant::now() < u).unwrap_or(false) {
                return false;
            }
        }
        true
    }

    async fn resolve(&self, query: &TrackQuery) -> Resolution {
        self.resolve_inner(query).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_item_shape() {
        // Documents the api.audius.co contract this provider relies on.
        let v = serde_json::json!({ "data": [
            { "id": "4x4BkyZ", "title": "Calm down", "duration": 63,
              "user": { "name": "BELLA240" } },
            { "id": "del1", "title": "Gone", "duration": 200, "is_delete": true }
        ] });
        let items = v["data"].as_array().unwrap();
        assert_eq!(items[0]["id"].as_str(), Some("4x4BkyZ"));
        assert_eq!(items[0]["duration"].as_u64(), Some(63));
        assert_eq!(items[0]["user"]["name"].as_str(), Some("BELLA240"));
    }

    #[test]
    fn pick_track_skips_deleted_and_mismatched() {
        let items = serde_json::json!([
            { "id": "gone", "title": "Gone", "duration": 200, "is_delete": true },
            { "id": "hour", "title": "Hour", "duration": 3600 },
            { "id": "hit1", "title": "Hit", "duration": 205 }
        ]);
        let arr = items.as_array().unwrap();
        // 200s track: skips deleted + hour-long, takes the match.
        assert_eq!(pick_track(arr, 200_000).as_deref(), Some("hit1"));
        // Unknown track duration accepts the first live entry.
        assert_eq!(pick_track(arr, 0).as_deref(), Some("hour"));
        // Nothing acceptable.
        let only = serde_json::json!([
            { "id": "x", "duration": 5 }
        ]);
        assert_eq!(pick_track(only.as_array().unwrap(), 200_000), None);
    }
}
