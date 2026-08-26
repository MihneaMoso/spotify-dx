//! Odesli (song.link) ID mapping: Spotify track → provider IDs.
//!
//! Odesli provides a free API that maps a track ID on one platform to
//! equivalent IDs on others. We use it to get TIDAL, Qobuz, and YouTube
//! IDs from a Spotify track ID.
//!
//! API: `GET https://api.song.link/v1-alpha.1/links?url=<spotify_url>&userCountry=US`
//!
//! Results are cached in-memory by Spotify track ID to avoid repeated lookups.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

/// Provider-specific IDs returned by Odesli.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderIds {
    pub tidal_url: Option<String>,
    pub qobuz_url: Option<String>,
    pub youtube_url: Option<String>,
    pub apple_music_url: Option<String>,
}

/// In-memory cache of Spotify track ID → provider IDs.
/// TTL: session lifetime (Odesli data is stable for released tracks).
static ODESLI_CACHE: OnceLock<std::sync::Mutex<HashMap<String, ProviderIds>>> =
    OnceLock::new();

fn cache() -> &'static std::sync::Mutex<HashMap<String, ProviderIds>> {
    ODESLI_CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

#[derive(Deserialize)]
struct OdesliResponse {
    #[serde(default, rename = "linksByPlatform")]
    links_by_platform: Option<HashMap<String, OdesliPlatform>>,
}

#[derive(Deserialize)]
struct OdesliPlatform {
    #[serde(default)]
    url: Option<String>,
}

/// Resolve Spotify track ID → provider IDs via Odesli.
///
/// Returns `None` on network failure or if the track has no mappings.
/// Cached after the first successful lookup.
pub async fn resolve(spotify_id: &str) -> Option<ProviderIds> {
    // Check cache first.
    {
        let map = cache().lock().ok()?;
        if let Some(ids) = map.get(spotify_id) {
            return Some(ids.clone());
        }
    }

    let spotify_url = format!("https://open.spotify.com/track/{spotify_id}");
    let api_url = format!(
        "https://api.song.link/v1-alpha.1/links?url={}&userCountry=US",
        urlencoding::encode(&spotify_url)
    );

    let resp = crate::spotify::client::filtered_get(&api_url).await.ok()?;
    let body = resp.text().await.ok()?;
    let parsed: OdesliResponse = serde_json::from_str(&body).ok()?;

    let platforms = parsed.links_by_platform?;

    let ids = ProviderIds {
        tidal_url: platforms
            .get("tidal")
            .and_then(|p| p.url.clone()),
        qobuz_url: platforms
            .get("qobuz")
            .and_then(|p| p.url.clone()),
        youtube_url: platforms
            .get("youtube")
            .and_then(|p| p.url.clone()),
        apple_music_url: platforms
            .get("appleMusic")
            .and_then(|p| p.url.clone()),
    };

    // Cache the result.
    if let Ok(mut map) = cache().lock() {
        map.insert(spotify_id.to_string(), ids.clone());
    }

    Some(ids)
}

/// Extract a platform-specific track ID from its Odesli URL.
/// E.g. "https://tidal.com/browse/12345" → "12345".
pub fn extract_id_from_url(url: &str) -> Option<String> {
    // TIDAL: https://tidal.com/browse/12345 or https://listen.tidal.com/track/12345
    // Qobuz: https://www.qobuz.com/us/en/album/abc-def-12345 (uses album, not track)
    // YouTube: https://www.youtube.com/watch?v=VIDEO_ID
    if let Some(vtid) = url.split("v=").nth(1) {
        // YouTube: strip any trailing params
        return Some(vtid.split('&').next().unwrap_or(vtid).to_string());
    }
    // Generic: last path segment
    url.rsplit('/').next().map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_youtube_id() {
        let url = "https://www.youtube.com/watch?v=dQw4w9WgXcQ&list=PLxyz";
        assert_eq!(extract_id_from_url(url).unwrap(), "dQw4w9WgXcQ");
    }

    #[test]
    fn extract_tidal_id() {
        let url = "https://tidal.com/browse/12345";
        assert_eq!(extract_id_from_url(url).unwrap(), "12345");
    }

    #[test]
    fn extract_generic_path() {
        let url = "https://example.com/track/abc123";
        assert_eq!(extract_id_from_url(url).unwrap(), "abc123");
    }

    #[test]
    fn provider_ids_default_is_empty() {
        let ids = ProviderIds::default();
        assert!(ids.tidal_url.is_none());
        assert!(ids.qobuz_url.is_none());
        assert!(ids.youtube_url.is_none());
    }
}
