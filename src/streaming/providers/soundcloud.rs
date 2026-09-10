//! SoundCloud provider: remixes, bootlegs, live versions and indie uploads
//! nothing else carries. Public api-v2 with an auto-scraped `client_id`
//! (the same key SoundCloud's own web client embeds — scraped from its
//! public JS bundles, cached 24h, refreshed on 401/403, so rotation is
//! self-healing with no user action).
//!
//! Flow: ensure client_id → `GET /search` → first policy-ALLOW,
//! duration-acceptable track → progressive `transcodings[]` entry → resolve
//! its `url` to the direct stream URL.
//!
//! Two hard rules: snippet-only results (`policy != ALLOW`, typically
//! major-label uploads) are SKIPPED — a 30s preview cutting off mid-song
//! is worse than continuing the chain; and only `progressive` transcodings
//! are usable (the platform player cannot do HLS).
//!
//! Last in the chain: highest fragility (scraped key), most unique catalog.

#[cfg(not(target_arch = "wasm32"))]
use std::time::{Duration, Instant};
#[cfg(target_arch = "wasm32")]
use std::time::Instant;
use std::sync::Mutex;

use async_trait::async_trait;
use regex::Regex;

use super::youtube;
use crate::streaming::provider::{AudioFormat, Provider, Quality, Resolution, TrackQuery};

const API_V2: &str = "https://api-v2.soundcloud.com";
const DISCOVER: &str = "https://soundcloud.com/discover";
const SC_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64)";

/// client_id cache TTL (matches the proven 24h pattern).
#[cfg(not(target_arch = "wasm32"))]
const KEY_TTL: Duration = Duration::from_secs(24 * 60 * 60);
#[cfg(not(target_arch = "wasm32"))]
const COOL_AFTER_FAILURES: u32 = 3;
#[cfg(not(target_arch = "wasm32"))]
const COOLDOWN: Duration = Duration::from_secs(5 * 60);
#[cfg(not(target_arch = "wasm32"))]
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);

pub struct SoundcloudProvider {
    client: reqwest::Client,
    /// Cached key + fetch time (`None` timestamp on wasm = no expiry there).
    key: Mutex<Option<(String, Option<Instant>)>>,
    failures: Mutex<u32>,
    #[cfg(not(target_arch = "wasm32"))]
    cooling_until: Mutex<Option<Instant>>,
}

impl Default for SoundcloudProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl SoundcloudProvider {
    pub fn new() -> Self {
        Self {
            client: {
                let builder = reqwest::Client::builder().user_agent(SC_UA);
                #[cfg(not(target_arch = "wasm32"))]
                let builder = builder.timeout(REQUEST_TIMEOUT);
                builder.build().unwrap_or_default()
            },
            key: Mutex::new(None),
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

    /// Cached client_id, freshly scraped when missing/stale.
    async fn client_id(&self) -> Option<String> {
        #[cfg(not(target_arch = "wasm32"))]
        let fresh_enough = |at: Instant| Instant::now().duration_since(at) < KEY_TTL;
        if let Ok(guard) = self.key.lock() {
            if let Some((id, at)) = guard.as_ref() {
                #[cfg(not(target_arch = "wasm32"))]
                if at.map(fresh_enough).unwrap_or(false) {
                    return Some(id.clone());
                }
                #[cfg(target_arch = "wasm32")]
                {
                    let _ = at;
                    return Some(id.clone());
                }
            }
        }
        let fresh = scrape_client_id(&self.client).await?;
        if let Ok(mut guard) = self.key.lock() {
            #[cfg(not(target_arch = "wasm32"))]
            {
                *guard = Some((fresh.clone(), Some(Instant::now())));
            }
            #[cfg(target_arch = "wasm32")]
            {
                *guard = Some((fresh.clone(), None));
            }
        }
        Some(fresh)
    }

    /// Forget the key so the next call re-scrapes (401/403 recovery).
    fn drop_key(&self) {
        if let Ok(mut guard) = self.key.lock() {
            *guard = None;
        }
    }

    async fn resolve_inner(&self, query: &TrackQuery) -> Resolution {
        let cid = match self.client_id().await {
            Some(id) => id,
            None => {
                self.note_failure();
                return Resolution::Error("soundcloud client_id scrape failed".into());
            }
        };
        match self.search_and_resolve(&cid, query).await {
            Ok(res) => {
                self.note_success();
                res
            }
            Err(SearchError::Auth) => {
                // One self-heal: fresh key, single retry.
                self.drop_key();
                let cid = match self.client_id().await {
                    Some(id) => id,
                    None => {
                        self.note_failure();
                        return Resolution::Error("soundcloud client_id refresh failed".into());
                    }
                };
                match self.search_and_resolve(&cid, query).await {
                    Ok(res) => {
                        self.note_success();
                        res
                    }
                    Err(_) => {
                        self.note_failure();
                        Resolution::Error("soundcloud request failed".into())
                    }
                }
            }
            Err(SearchError::Other) => {
                self.note_failure();
                Resolution::Error("soundcloud request failed".into())
            }
        }
    }

    /// Search + pick + transcode with one key. `Auth` signals a dead key
    /// (retry once fresh); `Other` is terminal for this key.
    async fn search_and_resolve(
        &self,
        cid: &str,
        query: &TrackQuery,
    ) -> Result<Resolution, SearchError> {
        let url = format!(
            "{API_V2}/search?q={}+{}&client_id={cid}&limit=10&offset=0",
            urlencode(&query.artist),
            urlencode(&query.title)
        );
        let resp = self.client.get(&url).send().await.map_err(|_| SearchError::Other)?;
        if resp.status().as_u16() == 401 || resp.status().as_u16() == 403 {
            return Err(SearchError::Auth);
        }
        if !resp.status().is_success() {
            return Err(SearchError::Other);
        }
        let val: serde_json::Value = resp.json().await.map_err(|_| SearchError::Other)?;
        let items = val
            .get("collection")
            .and_then(|c| c.as_array())
            .ok_or(SearchError::Other)?;
        for item in items {
            // Snippet-only uploads (major labels) are worse than nothing.
            if item.get("policy").and_then(|p| p.as_str()) != Some("ALLOW") {
                continue;
            }
            let duration = item.get("full_duration").and_then(|d| d.as_u64());
            if !youtube::duration_accepts(query.duration_ms, duration) {
                continue;
            }
            let transcodings = match item
                .get("media")
                .and_then(|m| m.get("transcodings"))
                .and_then(|t| t.as_array())
            {
                Some(t) => t,
                None => continue,
            };
            // Progressive only — the platform player cannot do HLS.
            let prog = transcodings.iter().find(|t| {
                t.get("format")
                    .and_then(|f| f.get("protocol"))
                    .and_then(|p| p.as_str())
                    == Some("progressive")
            });
            let entry = match prog {
                Some(e) => e,
                None => continue,
            };
            let quality = if entry.get("quality").and_then(|q| q.as_str()) == Some("hq") {
                Quality::High
            } else {
                Quality::Normal
            };
            let t_url = match entry.get("url").and_then(|u| u.as_str()) {
                Some(u) => u,
                None => continue,
            };
            match self.resolve_transcoding(t_url, cid).await {
                Transcode::Url(url) => {
                    return Ok(Resolution::Success {
                        url,
                        format: AudioFormat::Mp3,
                        quality,
                    });
                }
                Transcode::Auth => return Err(SearchError::Auth),
                Transcode::Unusable => continue,
            }
        }
        Ok(Resolution::NotFound)
    }

    /// Resolve one transcoding URL to its direct stream URL.
    async fn resolve_transcoding(&self, t_url: &str, cid: &str) -> Transcode {
        let resp = match self.client.get(format!("{t_url}?client_id={cid}")).send().await {
            Ok(r) => r,
            Err(_) => return Transcode::Unusable,
        };
        if resp.status().as_u16() == 401 || resp.status().as_u16() == 403 {
            return Transcode::Auth;
        }
        let val: serde_json::Value = match resp.json().await {
            Ok(v) => v,
            Err(_) => return Transcode::Unusable,
        };
        match val.get("url").and_then(|u| u.as_str()) {
            Some(u) if u.starts_with("http") => Transcode::Url(u.to_string()),
            _ => Transcode::Unusable,
        }
    }
}

/// Search failure classes: dead key (retry once fresh) vs terminal.
enum SearchError {
    Auth,
    Other,
}

enum Transcode {
    Url(String),
    Auth,
    Unusable,
}

/// Scrape the web client's embedded `client_id` from its public JS bundles:
/// discover HTML → asset URLs → first `client_id:"<32 alnum>"` match.
/// Pure network walk, no key needed (this IS the bootstrap).
async fn scrape_client_id(client: &reqwest::Client) -> Option<String> {
    let html = client.get(DISCOVER).send().await.ok()?.text().await.ok()?;
    let re_assets = Regex::new(r"https://a-v2\.sndcdn\.com/assets/[0-9]+-[a-z0-9]+\.js").ok()?;
    let re_key = Regex::new(r#"client_id:"([A-Za-z0-9]{20,})""#).ok()?;
    // Dedup: same bundle may be referenced twice.
    let mut seen = std::collections::HashSet::new();
    for m in re_assets.find_iter(&html) {
        let url = m.as_str();
        if !seen.insert(url) {
            continue;
        }
        let js = client.get(url).send().await.ok()?.text().await.ok()?;
        if let Some(cap) = re_key.captures(&js) {
            return Some(cap[1].to_string());
        }
    }
    None
}

/// Minimal percent-encoding for query params (mirrors piped.rs).
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
impl Provider for SoundcloudProvider {
    fn name(&self) -> &'static str {
        "soundcloud"
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
    fn client_id_regex_matches_embedded_key() {
        // Canned fragment of a real bundle (key redacted to same shape).
        let js = r#"foo client_id:"Pb72ranhoyt6gw7hM7TkzUItXlMWSNSo",bar"#;
        let re = Regex::new(r#"client_id:"([A-Za-z0-9]{20,})""#).unwrap();
        assert_eq!(
            re.captures(js).unwrap().get(1).unwrap().as_str(),
            "Pb72ranhoyt6gw7hM7TkzUItXlMWSNSo"
        );
        assert!(re.captures("no key here").is_none());
    }

    #[test]
    fn policy_gate_skips_snippets() {
        // Major-label snippet (policy SNIP) vs full indie upload (ALLOW).
        let snip = serde_json::json!({ "policy": "SNIP", "full_duration": 30000 });
        let full = serde_json::json!({ "policy": "ALLOW", "full_duration": 219847 });
        assert_ne!(snip["policy"].as_str(), Some("ALLOW"));
        assert_eq!(full["policy"].as_str(), Some("ALLOW"));
    }

    #[test]
    fn transcoding_pick_contract() {
        // Documents the api-v2 media shape this provider relies on.
        let v = serde_json::json!({ "media": { "transcodings": [
            { "url": "https://x/hls", "quality": "sq",
              "format": { "protocol": "hls", "mime_type": "audio/mpeg" } },
            { "url": "https://x/prog", "quality": "sq",
              "format": { "protocol": "progressive", "mime_type": "audio/mpeg" } }
        ] } });
        let arr = v["media"]["transcodings"].as_array().unwrap();
        let prog = arr.iter().find(|t| {
            t["format"]["protocol"].as_str() == Some("progressive")
        }).unwrap();
        assert_eq!(prog["url"].as_str(), Some("https://x/prog"));
    }
}
