//! TIDAL provider — PARKED, stub only.
//!
//! `is_available()` is unconditionally false: Odesli (song.link) — the only
//! Spotify→TIDAL ID mapper — was sunset, and the community proxy instances
//! return 404. A `tidal_token` settings field exists, but a
//! direct-search+stream revival needs real credentials to verify against
//! (endpoint shapes, token type, manifest parsing) — deliberately NOT
//! shipped blind.
//!
//! What was here (uptime-list pool, instance probing, proxy resolve) was
//! removed as untestable live machinery; it lives in git history. Revival
//! notes: map IDs without Odesli (direct search with the token), verify a
//! manifest/stream URL shape against a real account, then re-add resolve +
//! health gating behind the existing `is_available` switch. The chain
//! already skips parked providers without network calls.
//!
//! Kept: the pure response helpers below (unit-tested, reusable on revival).

use async_trait::async_trait;

#[cfg(test)]
use crate::streaming::provider::AudioFormat;
use crate::streaming::provider::{Provider, Resolution, TrackQuery};

/// Parked provider handle: zero state (no client, no pools) since nothing
/// here touches the network.
pub struct TidalProvider;

impl Default for TidalProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl TidalProvider {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait(?Send)]
impl Provider for TidalProvider {
    fn name(&self) -> &'static str {
        "tidal"
    }

    fn is_available(&self) -> bool {
        // STILL PARKED (Phase F assessment): Odesli (song.link) — the only
        // Spotify→TIDAL ID mapper in this file — was sunset, and the
        // community proxy instances return 404. A `tidal_token` settings
        // field now exists, but a direct-search+stream revival needs real
        // credentials to verify against (endpoint shapes, token type,
        // manifest parsing) — deliberately NOT shipped blind. Until then it
        // returns false so the resolver skips it without network calls.
        false
    }

    async fn resolve(&self, _query: &TrackQuery) -> Resolution {
        // Parked: no ID mapper, no verified stream shape. Anything here
        // would be untestable guesswork against dead endpoints.
        Resolution::NotFound
    }
}

/// Pure response helpers, kept unit-tested for a future revival (they
/// exercise no network). Non-test builds don't reference them — that is
/// intentional while the provider is parked, not dead code to "clean up".
#[cfg(test)]
fn extract_url_from_response(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.starts_with("http") {
        return trimmed.to_string();
    }
    // Try JSON: {"url": "..."}
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if let Some(url) = val.get("url").and_then(|u| u.as_str()) {
            return url.to_string();
        }
    }
    String::new()
}

/// Guess the audio format from a URL's file extension (test-only while
/// parked — see above).
#[cfg(test)]
fn guess_format(url: &str) -> AudioFormat {
    let lower = url.to_lowercase();
    if lower.contains(".flac") {
        AudioFormat::Flac
    } else if lower.contains(".mp3") {
        AudioFormat::Mp3
    } else if lower.contains(".m4a") || lower.contains(".aac") {
        AudioFormat::Aac
    } else if lower.contains(".ogg") || lower.contains(".opus") {
        AudioFormat::Ogg
    } else {
        AudioFormat::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_url_plain() {
        assert_eq!(
            extract_url_from_response("https://cdn.example.com/song.flac"),
            "https://cdn.example.com/song.flac"
        );
    }

    #[test]
    fn extract_url_json() {
        let body = r#"{"url": "https://cdn.example.com/song.flac", "quality": "hires"}"#;
        assert_eq!(
            extract_url_from_response(body),
            "https://cdn.example.com/song.flac"
        );
    }

    #[test]
    fn guess_format_from_extension() {
        assert_eq!(guess_format("https://x.com/s.flac"), AudioFormat::Flac);
        assert_eq!(guess_format("https://x.com/s.mp3"), AudioFormat::Mp3);
        assert_eq!(guess_format("https://x.com/s.m4a"), AudioFormat::Aac);
        assert_eq!(guess_format("https://x.com/s"), AudioFormat::Unknown);
    }
}
