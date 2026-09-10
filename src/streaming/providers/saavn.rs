//! JioSaavn provider: first-party API, 320kbps MP3, no key.
//!
//! Talks to JioSaavn's own `api.php` directly (NOT third-party wrapper
//! deployments — those are rate-limited demos that come and go):
//! `search.getResults` → pick → decrypt `encrypted_media_url` (DES-ECB with
//! the API's public static key, same scheme every client uses) → swap the
//! `_96` quality token for `_320`.
//!
//! Sits after `piped` in the chain: distinct catalog strength (Indian +
//! international mainstream, deep back catalog) for misses the YouTube
//! family can't cover. Skips disabled/unavailable items (`disabled`,
//! `rights.code`) and duration-gates like every other provider.
//! `explicit_content` is informational only — never filtered.

use std::sync::Mutex;
#[cfg(not(target_arch = "wasm32"))]
use std::time::{Duration, Instant};

use async_trait::async_trait;
use cipher::{BlockDecrypt, KeyInit};
use des::Des;

use super::youtube;
use crate::streaming::provider::{AudioFormat, Provider, Quality, Resolution, TrackQuery};

/// First-party search. `n_song` bounds the payload; zero album/artist/
/// playlist buckets (songs only — this provider resolves tracks).
const SEARCH_URL: &str = "https://www.jiosaavn.com/api.php?__call=search.getResults\
    &_marker=0&_format=json&p=1&n_album=0&n_artist=0&n_playlist=0&n_song=8";

/// DES-ECB key for `encrypted_media_url` (ASCII, the API's public key —
/// see the maintained jiosaavn-api reference implementation).
const DES_KEY: &[u8] = b"38346591";

/// Consecutive failures before the provider cools down (single host — no
/// pool to rotate, so this only gates total-outage storms).
const COOL_AFTER_FAILURES: u32 = 3;
#[cfg(not(target_arch = "wasm32"))]
const COOLDOWN: Duration = Duration::from_secs(5 * 60);
#[cfg(not(target_arch = "wasm32"))]
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);

pub struct SaavnProvider {
    client: reqwest::Client,
    failures: Mutex<u32>,
    #[cfg(not(target_arch = "wasm32"))]
    cooling_until: Mutex<Option<Instant>>,
}

impl Default for SaavnProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl SaavnProvider {
    pub fn new() -> Self {
        Self {
            client: {
                let builder = reqwest::Client::builder()
                    .user_agent("Mozilla/5.0 (Linux; Android 11)");
                #[cfg(not(target_arch = "wasm32"))]
                let builder = builder.timeout(REQUEST_TIMEOUT);
                builder.build().unwrap_or_default()
            },
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
                    *c = Some(Instant::now() + COOLDOWN);
                }
            }
        }
    }

    async fn resolve_inner(&self, query: &TrackQuery) -> Resolution {
        let url = format!(
            "{SEARCH_URL}&q={}+{}",
            urlencode(&query.artist),
            urlencode(&query.title)
        );
        let resp = match self.client.get(&url).send().await {
            Ok(r) => r,
            Err(_) => {
                self.note_failure();
                return Resolution::Error("saavn search request failed".into());
            }
        };
        let val: serde_json::Value = match resp.json().await {
            Ok(v) => v,
            Err(_) => {
                self.note_failure();
                return Resolution::Error("saavn search parse failed".into());
            }
        };
        let results = match val.get("results").and_then(|r| r.as_array()) {
            Some(r) => r,
            None => {
                self.note_failure();
                return Resolution::Error("saavn search shape changed".into());
            }
        };
        for item in results {
            if !playable_item(item) {
                continue;
            }
            let duration = item
                .get("duration")
                .and_then(|d| d.as_str())
                .and_then(|s| s.parse::<u64>().ok());
            match duration {
                // Duration present: gate (also filters hour-long compilations).
                Some(secs) if !youtube::duration_accepts(query.duration_ms, Some(secs)) => {
                    continue;
                }
                // Duration missing but track unknown too: accept. Duration
                // missing while we know it: require it (Saavn always sends
                // durations — missing means a degenerate entry).
                None if query.duration_ms != 0 => continue,
                _ => {}
            }
            let enc = match item.get("encrypted_media_url").and_then(|u| u.as_str()) {
                Some(u) if !u.is_empty() => u,
                _ => continue,
            };
            let base = match decrypt_media_url(enc) {
                Some(u) => u,
                None => continue,
            };
            // Prefer 320kbps when the item advertises it, else 160.
            let high = item.get("320kbps").and_then(|v| v.as_str()) == Some("true");
            let token = if high { "_320" } else { "_160" };
            let stream_url = base.replacen("_96", token, 1);
            self.note_success();
            return Resolution::Success {
                url: stream_url,
                format: AudioFormat::Unknown,
                quality: Quality::High,
            };
        }
        Resolution::NotFound
    }
}

/// Availability gate: item-level rights, not just presence.
/// Skips label-withdrawn entries (`disabled`, non-zero `rights.code`) that
/// would 403/404 at playback. Pure (unit-tested).
fn playable_item(item: &serde_json::Value) -> bool {
    if item.get("disabled").and_then(|d| d.as_str()) == Some("true") {
        return false;
    }
    if let Some(code) = item.get("rights").and_then(|r| r.get("code")) {
        let blocked = match code {
            serde_json::Value::Number(n) => n.as_u64().unwrap_or(0) != 0,
            serde_json::Value::String(s) => s != "0",
            _ => false,
        };
        if blocked {
            return false;
        }
    }
    true
}

/// Decrypt `encrypted_media_url`: base64 → DES-ECB → PKCS7 strip.
/// Pure (unit-tested with a fixed live vector).
fn decrypt_media_url(enc: &str) -> Option<String> {
    use base64::Engine as _;
    let mut bytes = base64::engine::general_purpose::STANDARD
        .decode(enc.trim())
        .ok()?;
    if bytes.is_empty() || bytes.len() % 8 != 0 {
        return None;
    }
    let cipher = Des::new_from_slice(DES_KEY).ok()?;
    for chunk in bytes.chunks_exact_mut(8) {
        cipher.decrypt_block(chunk.into());
    }
    // PKCS7 unpad (node-forge `finish()` equivalent).
    let pad = *bytes.last()? as usize;
    if pad == 0 || pad > 8 || pad > bytes.len() {
        return None;
    }
    bytes.truncate(bytes.len() - pad);
    String::from_utf8(bytes).ok()
}

/// Minimal percent-encoding for query params (mirrors piped.rs; no new deps).
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
impl Provider for SaavnProvider {
    fn name(&self) -> &'static str {
        "saavn"
    }

    fn is_available(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let cooling = self.cooling_until.lock().unwrap_or_else(|e| e.into_inner());
            if cooling.map(|u| Instant::now() < u).unwrap_or(false) {
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

    /// Fixed live vector: ciphertext → plaintext (verified against the
    /// reference implementation's algorithm before pinning).
    const ENC: &str = "ID2ieOjCrwfgWvL5sXl4B1ImC5QfbsDyZRQ8Cw47oXXrYT0nSLbsp8fEBK8oXFmBcZC7w9u1j6o+CSJO2tsw/hw7tS9a8Gtq";
    const PLAIN: &str = "https://aac.saavncdn.com/366/fcbf0d7acd7f132746ce655f8b4237b9_96.mp4";

    #[test]
    fn decrypt_media_url_recovers_stream_template() {
        assert_eq!(decrypt_media_url(ENC).as_deref(), Some(PLAIN));
        assert!(decrypt_media_url("").is_none());
        assert!(decrypt_media_url("!!!not-base64!!!").is_none());
        // Quality token swap reaches 320kbps.
        assert_eq!(
            PLAIN.replacen("_96", "_320", 1),
            "https://aac.saavncdn.com/366/fcbf0d7acd7f132746ce655f8b4237b9_320.mp4"
        );
    }

    #[test]
    fn playable_item_skips_withdrawn_entries() {
        assert!(playable_item(&serde_json::json!({})));
        assert!(playable_item(
            &serde_json::json!({ "disabled": "false", "rights": { "code": 0 } })
        ));
        assert!(!playable_item(&serde_json::json!({ "disabled": "true" })));
        assert!(!playable_item(
            &serde_json::json!({ "rights": { "code": 1, "reason": "Unavailable" } })
        ));
        assert!(!playable_item(
            &serde_json::json!({ "rights": { "code": "1" } })
        ));
    }

    #[test]
    fn search_item_shape() {
        // Documents the first-party api.php contract this provider relies on.
        let v = serde_json::json!({ "results": [
            { "id": "x", "song": "S", "duration": "207",
              "encrypted_media_url": ENC, "320kbps": "true" }
        ] });
        let item = &v["results"][0];
        assert!(playable_item(item));
        assert_eq!(item["duration"].as_str().unwrap().parse::<u64>().unwrap(), 207);
        assert!(decrypt_media_url(item["encrypted_media_url"].as_str().unwrap()).is_some());
    }
}
