//! High-level stream resolver: Odesli mapping → provider failover → URL cache.
//!
//! The resolver is the main entry point for the open streaming engine.
//! It orchestrates the full pipeline:
//! 1. Check the stream-URL cache first.
//! 2. Build a `TrackQuery` from Spotify metadata.
//! 3. Try providers in order with cooldown-aware failover.
//! 4. Cache the result.

use crate::streaming::cache;
use crate::streaming::provider::{Resolution, TrackQuery};
use crate::streaming::providers;

/// Result of resolving a track to a playable URL.
#[derive(Debug, Clone)]
pub struct ResolvedStream {
    pub url: String,
    pub format: crate::streaming::provider::AudioFormat,
    pub quality: crate::streaming::provider::Quality,
    pub provider: String,
}

/// Resolve a Spotify track to a playable audio URL.
///
/// Checks the cache first, then tries the provider chain in order.
/// Returns `Err` only on hard failure; `Ok(None)` means "not found anywhere".
pub async fn resolve(track: &crate::spotify::models::Track) -> Result<Option<ResolvedStream>, String> {
    let track_id = &track.id;

    // Step 1: Check cache.
    for provider_name in &["tidal", "qobuz", "youtube"] {
        if let Some(cached) = cache::get(track_id, provider_name) {
            if !cached.is_expired() {
                tracing::debug!(
                    "stream cache hit for {track_id} on {provider_name}"
                );
                let format = match cached.format.as_str() {
                    "flac" => crate::streaming::provider::AudioFormat::Flac,
                    "mp3" => crate::streaming::provider::AudioFormat::Mp3,
                    "m4a" | "aac" => crate::streaming::provider::AudioFormat::Aac,
                    "ogg" => crate::streaming::provider::AudioFormat::Ogg,
                    _ => crate::streaming::provider::AudioFormat::Unknown,
                };
                return Ok(Some(ResolvedStream {
                    url: cached.url,
                    format,
                    quality: crate::streaming::provider::Quality::Lossless,
                    provider: provider_name.to_string(),
                }));
            }
        }
    }

    // Step 2: Build the track query.
    let query = build_query(track);

    // Step 3: Try providers in order.
    let chain = providers::build_provider_chain();
    for provider in &chain {
        if !provider.is_available() {
            tracing::debug!("provider {} unavailable, skipping", provider.name());
            continue;
        }
        tracing::debug!("trying provider: {}", provider.name());
        match provider.resolve(&query).await {
            Resolution::Success { url, format, quality } => {
                tracing::info!(
                    "resolved {track_id} via {} → {format:?} {quality:?}",
                    provider.name()
                );
                // Cache the result.
                cache::put(track_id, provider.name(), &url, &format.to_string());
                return Ok(Some(ResolvedStream {
                    url,
                    format,
                    quality,
                    provider: provider.name().to_string(),
                }));
            }
            Resolution::Cooldown { retry_after_secs } => {
                tracing::warn!(
                    "provider {} on cooldown ({retry_after_secs}s), trying next",
                    provider.name()
                );
                // Don't wait — just move to the next provider.
                continue;
            }
            Resolution::NotFound => {
                tracing::debug!("provider {} not found for {track_id}", provider.name());
                continue;
            }
            Resolution::Error(e) => {
                tracing::warn!("provider {} error: {e}", provider.name());
                continue;
            }
        }
    }

    Ok(None) // No provider could resolve this track.
}

/// Build a `TrackQuery` from Spotify track metadata.
fn build_query(track: &crate::spotify::models::Track) -> TrackQuery {
    let artist = track
        .artists
        .first()
        .map(|a| a.name.clone())
        .unwrap_or_default();
    let album = track.album.name.clone();
    let album_isrc = None; // ISRC would come from album detail API if available.
    TrackQuery {
        spotify_id: track.id.clone(),
        isrc: album_isrc,
        title: track.name.clone(),
        artist,
        album: if album.is_empty() { None } else { Some(album) },
        duration_ms: track.duration_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spotify::models::{AlbumRef, ArtistRef, Track};

    fn mk_track(id: &str, name: &str, artist: &str) -> Track {
        Track {
            id: id.to_string(),
            name: name.to_string(),
            uri: format!("spotify:track:{id}"),
            duration_ms: 200_000,
            explicit: false,
            artists: vec![ArtistRef {
                id: "a1".into(),
                name: artist.to_string(),
                uri: "spotify:artist:a1".into(),
            }],
            album: AlbumRef {
                id: "al1".into(),
                name: "Test Album".into(),
                uri: "spotify:album:al1".into(),
                images: vec![],
                album_type: None,
                release_date: None,
            },
            preview_url: None,
            popularity: 50,
        }
    }

    #[test]
    fn build_query_from_track() {
        let track = mk_track("abc123", "Test Song", "Test Artist");
        let q = build_query(&track);
        assert_eq!(q.spotify_id, "abc123");
        assert_eq!(q.title, "Test Song");
        assert_eq!(q.artist, "Test Artist");
        assert_eq!(q.album, Some("Test Album".to_string()));
        assert_eq!(q.duration_ms, 200_000);
    }

    #[test]
    fn build_query_no_artists() {
        let mut track = mk_track("x", "Song", "");
        track.artists.clear();
        let q = build_query(&track);
        assert_eq!(q.artist, "");
    }
}
