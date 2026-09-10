//! Qobuz provider: lossless FLAC via user credentials.
//!
//! Revived (Phase F) behind user-supplied credentials (`qobuz_app_id` +
//! `qobuz_auth_token` in Settings; empty = parked, zero cost). Flow:
//! 1. ISRC search when the query carries one (most accurate), else
//!    title+artist text search — both with `app_id` + `user_auth_token`.
//! 2. `track/getFileUrl` (format 6 = CD FLAC) for the first hit's numeric
//!    track id → direct stream URL.
//!
//! LIVE-VERIFY PENDING: written against the documented Qobuz API surface,
//! but no user keys were available to confirm search params, `streamable`
//! gating, or `getFileUrl` response shape on-device. The miss-path ISRC
//! retry only runs when configured, so unverified code never executes
//! without keys. Verify with real credentials before trusting: ISRC hit →
//! FLAC URL → playback.

#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

use async_trait::async_trait;
use dioxus::prelude::ReadableExt;

use super::youtube;
use crate::streaming::provider::{AudioFormat, Provider, Quality, Resolution, TrackQuery};

const API: &str = "https://api.qobuz.com/api/2.0";
/// CD-quality FLAC (16/44.1): the most widely available lossless tier.
/// Higher tiers (24-bit) fail per-track far more often.
const FORMAT_CD_FLAC: u8 = 6;

#[cfg(not(target_arch = "wasm32"))]
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

pub struct QobuzProvider {
    client: reqwest::Client,
}

impl Default for QobuzProvider {
    fn default() -> Self {
        Self::new()
    }
}

/// Configured credentials, if the user supplied both halves.
pub fn credentials() -> Option<(String, String)> {
    let s = crate::state::SETTINGS.read();
    if s.qobuz_app_id.is_empty() || s.qobuz_auth_token.is_empty() {
        None
    } else {
        Some((s.qobuz_app_id.clone(), s.qobuz_auth_token.clone()))
    }
}

/// Whether the provider may run (resolver miss-path hook consults this
/// before spending a MusicBrainz lookup).
pub fn is_configured() -> bool {
    credentials().is_some()
}

impl QobuzProvider {
    pub fn new() -> Self {
        Self {
            client: {
                let builder = reqwest::Client::builder();
                #[cfg(not(target_arch = "wasm32"))]
                let builder = builder.timeout(REQUEST_TIMEOUT);
                builder.build().unwrap_or_default()
            },
        }
    }

    /// Search tracks (ISRC string or `title artist` text); first hit's
    /// numeric track id + title for duration gating.
    async fn search_track_id(
        &self,
        app_id: &str,
        token: &str,
        query: &str,
    ) -> Result<Option<(u64, String, u64)>, SearchError> {
        let url = format!(
            "{API}/track/search?query={}&limit=5&app_id={app_id}&user_auth_token={token}",
            urlencoding::encode(query)
        );
        let resp = self.client.get(&url).send().await.map_err(|_| SearchError::Other)?;
        match resp.status().as_u16() {
            401 | 403 => return Err(SearchError::Auth),
            429 | 503 => return Err(SearchError::Cooldown),
            s if !(200..300).contains(&s) => return Err(SearchError::Other),
            _ => {}
        }
        let val: serde_json::Value = resp.json().await.map_err(|_| SearchError::Other)?;
        let items = val
            .get("tracks")
            .and_then(|t| t.get("items"))
            .and_then(|i| i.as_array())
            .ok_or(SearchError::Other)?;
        for item in items {
            let id = match item.get("id").and_then(|i| i.as_u64()) {
                Some(id) => id,
                None => continue,
            };
            let title = item.get("title").and_then(|t| t.as_str()).unwrap_or("").to_string();
            // Qobuz reports duration in SECONDS; 0 = unknown.
            let secs = item.get("duration").and_then(|d| d.as_u64()).unwrap_or(0);
            return Ok(Some((id, title, secs)));
        }
        Ok(None)
    }

    /// Direct file URL for a numeric track id (CD FLAC).
    async fn file_url(
        &self,
        app_id: &str,
        token: &str,
        track_id: u64,
    ) -> Result<Option<String>, SearchError> {
        let url = format!(
            "{API}/track/getFileUrl?format_id={FORMAT_CD_FLAC}\
             &track_id={track_id}&app_id={app_id}&user_auth_token={token}"
        );
        let resp = self.client.get(&url).send().await.map_err(|_| SearchError::Other)?;
        match resp.status().as_u16() {
            401 | 403 => return Err(SearchError::Auth),
            429 | 503 => return Err(SearchError::Cooldown),
            s if !(200..300).contains(&s) => return Err(SearchError::Other),
            _ => {}
        }
        let val: serde_json::Value = resp.json().await.map_err(|_| SearchError::Other)?;
        Ok(val
            .get("url")
            .and_then(|u| u.as_str())
            .filter(|u| u.starts_with("http"))
            .map(|u| u.to_string()))
    }

    async fn resolve_inner(
        &self,
        app_id: &str,
        token: &str,
        query: &TrackQuery,
    ) -> Resolution {
        // ISRC first (exact recording match), then text fallback.
        let mut attempts = Vec::new();
        if let Some(ref isrc) = query.isrc {
            attempts.push(isrc.clone());
        }
        attempts.push(format!("{} {}", query.title, query.artist));
        for q in attempts {
            let hit = match self.search_track_id(app_id, token, &q).await {
                Ok(h) => h,
                Err(SearchError::Auth) => {
                    return Resolution::Error("qobuz credentials rejected".into())
                }
                Err(SearchError::Cooldown) => {
                    return Resolution::Cooldown { retry_after_secs: 60 }
                }
                Err(SearchError::Other) => continue,
            };
            let Some((id, _title, secs)) = hit else {
                continue;
            };
            if secs != 0 && !youtube::duration_accepts(query.duration_ms, Some(secs)) {
                continue;
            }
            match self.file_url(app_id, token, id).await {
                Ok(Some(url)) => {
                    return Resolution::Success {
                        url,
                        format: AudioFormat::Flac,
                        quality: Quality::Lossless,
                    }
                }
                Ok(None) => continue,
                Err(SearchError::Auth) => {
                    return Resolution::Error("qobuz credentials rejected".into())
                }
                Err(SearchError::Cooldown) => {
                    return Resolution::Cooldown { retry_after_secs: 60 }
                }
                Err(SearchError::Other) => continue,
            }
        }
        Resolution::NotFound
    }
}

enum SearchError {
    Auth,
    Cooldown,
    Other,
}

#[async_trait(?Send)]
impl Provider for QobuzProvider {
    fn name(&self) -> &'static str {
        "qobuz"
    }

    fn is_available(&self) -> bool {
        // Parked without user credentials (zero cost); revived by Settings.
        is_configured()
    }

    async fn resolve(&self, query: &TrackQuery) -> Resolution {
        match credentials() {
            Some((app_id, token)) => self.resolve_inner(&app_id, &token, query).await,
            None => Resolution::NotFound,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_hit_shape() {
        // Documents the api.qobuz.com contract this provider relies on
        // (LIVE-VERIFY PENDING with real credentials).
        let v = serde_json::json!({ "tracks": { "items": [
            { "id": 123456, "title": "Mercy", "duration": 267 },
            { "title": "No id here" }
        ] } });
        let items = v["tracks"]["items"].as_array().unwrap();
        assert_eq!(items[0]["id"].as_u64(), Some(123456));
        assert_eq!(items[0]["duration"].as_u64(), Some(267));
        assert!(youtube::duration_accepts(267_111, Some(267)));
        assert!(!youtube::duration_accepts(267_111, Some(30)));
    }

    #[test]
    fn file_url_shape() {
        let v = serde_json::json!({ "url": "https://streaming.qobuz.com/file?uid=1" });
        assert_eq!(
            v["url"].as_str().filter(|u| u.starts_with("http")),
            Some("https://streaming.qobuz.com/file?uid=1")
        );
    }
}
