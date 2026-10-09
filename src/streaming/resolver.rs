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

/// Negative provider cache: (track_id, provider) pairs whose last
/// verdict was NotFound, with expiry. A provider that deterministically
/// lacks a track must not be re-asked on every play — each retry burns
/// seconds per resolve and hammers struggling instances for a known
/// answer. Recorded ONLY on NotFound (a content verdict); transport
/// errors and cooldowns are transient by nature and never enter here
/// (provider-level cooldowns already cover those). TTL bounds staleness
/// if content appears later. Skips the key entirely when empty (unkeyed
/// streams must never share negative state). Native only (needs a clock).
#[cfg(not(target_arch = "wasm32"))]
const NEGATIVE_TTL: std::time::Duration = std::time::Duration::from_secs(6 * 3600);
/// Bound (entries are tiny; purge runs on insert).
#[cfg(not(target_arch = "wasm32"))]
const NEGATIVE_CAP: usize = 2000;

#[cfg(not(target_arch = "wasm32"))]
struct NegativeCache {
    ttl: std::time::Duration,
    map: std::collections::HashMap<(String, String), std::time::Instant>,
}

#[cfg(not(target_arch = "wasm32"))]
impl NegativeCache {
    fn new(ttl: std::time::Duration) -> Self {
        Self {
            ttl,
            map: std::collections::HashMap::new(),
        }
    }

    /// True when this provider was a recent deterministic miss for track.
    fn skip(&self, track_id: &str, provider: &str, now: std::time::Instant) -> bool {
        if track_id.is_empty() {
            return false;
        }
        match self.map.get(&(track_id.to_string(), provider.to_string())) {
            Some(&exp) => exp > now,
            None => false,
        }
    }

    /// Record a deterministic miss; purges expired entries past the cap.
    fn record(&mut self, track_id: &str, provider: &str, now: std::time::Instant) {
        if track_id.is_empty() {
            return;
        }
        if self.map.len() >= NEGATIVE_CAP {
            self.map.retain(|_, exp| *exp > now);
        }
        self.map.insert(
            (track_id.to_string(), provider.to_string()),
            now + self.ttl,
        );
    }
}

#[cfg(not(target_arch = "wasm32"))]
static NEGATIVE: std::sync::OnceLock<std::sync::Mutex<NegativeCache>> = std::sync::OnceLock::new();

#[cfg(not(target_arch = "wasm32"))]
fn negative() -> &'static std::sync::Mutex<NegativeCache> {
    NEGATIVE.get_or_init(|| std::sync::Mutex::new(NegativeCache::new(NEGATIVE_TTL)))
}

/// Resolve a Spotify track to a playable audio URL.
///
/// Checks the cache first, then tries the provider chain in order.
/// `quality_hint` is the user's advisory ceiling (wifi vs metered
/// preference); None = unconstrained (legacy behavior). Hints only ever
/// narrow variant choice — never cause NotFound.
/// Returns `Err` only on hard failure; `Ok(None)` means "not found anywhere".
pub async fn resolve(
    track: &crate::spotify::models::Track,
    quality_hint: Option<crate::streaming::provider::Quality>,
) -> Result<Option<ResolvedStream>, String> {
    let track_id = &track.id;

    // Step 1: Check cache (every chain provider — see CACHE_PROBE_ORDER;
    // a hardcoded subset would leave some providers' entries write-only).
    for provider_name in providers::CACHE_PROBE_ORDER {
        if let Some(cached) = cache::get(track_id, provider_name) {
            if !cached.is_expired() {
                tracing::debug!("stream cache hit for {track_id} on {provider_name}");
                let format = match cached.format.as_str() {
                    "flac" => crate::streaming::provider::AudioFormat::Flac,
                    "mp3" => crate::streaming::provider::AudioFormat::Mp3,
                    "m4a" | "aac" => crate::streaming::provider::AudioFormat::Aac,
                    "ogg" => crate::streaming::provider::AudioFormat::Ogg,
                    "opus" => crate::streaming::provider::AudioFormat::Opus,
                    _ => crate::streaming::provider::AudioFormat::Unknown,
                };
                // Persisted tier round-trips (pre-quality entries land
                // Normal = honest unknown, never assumed lossless).
                let quality = match cached.quality.as_str() {
                    "low" => crate::streaming::provider::Quality::Low,
                    "high" => crate::streaming::provider::Quality::High,
                    "lossless" => crate::streaming::provider::Quality::Lossless,
                    _ => crate::streaming::provider::Quality::Normal,
                };
                return Ok(Some(ResolvedStream {
                    url: cached.url,
                    format,
                    quality,
                    provider: provider_name.to_string(),
                }));
            }
        }
    }

    // Step 2: Build the track query.
    let mut query = build_query(track);
    query.max_quality = quality_hint;

    // Step 3: Try providers in ranked order (tiers, then live score).
    // Cold order == the static chain (stable sort over equal scores), so
    // first-ever resolves behave bit-identically to before; scores only
    // diverge with live evidence. Scoring is passive, decay runs on its
    // own 60s timer — neither blocks nor performs I/O here.
    crate::streaming::ranking::ensure_decay_task();
    let chain = providers::build_provider_chain();
    let snapshot = crate::streaming::ranking::snapshot();
    let empty_stats = crate::streaming::ranking::ProviderStats::default();
    let mut legs: Vec<&Box<dyn crate::streaming::provider::Provider>> =
        chain.iter().collect();
    legs.sort_by_key(|p| {
        crate::streaming::ranking::rank_key(
            p.name(),
            snapshot.get(p.name()).unwrap_or(&empty_stats),
        )
    });
    for provider in legs {
        if !provider.is_available() {
            tracing::debug!("provider {} unavailable, skipping", provider.name());
            continue;
        }
        // Negative cache: a recent deterministic miss for THIS track
        // skips the provider (fallthrough continues below regardless).
        #[cfg(not(target_arch = "wasm32"))]
        if negative()
            .lock()
            .map(|n| n.skip(track_id, provider.name(), std::time::Instant::now()))
            .unwrap_or(false)
        {
            tracing::debug!(
                "provider {} negative-cached for {track_id}, skipping",
                provider.name()
            );
            continue;
        }
        tracing::debug!("trying provider: {}", provider.name());
        // Per-leg budget (native only): a stalled leg aborts into the
        // next provider instead of holding the resolve hostage. Counts
        // as a failure for scoring (no latency sample — a stall must not
        // normalize itself into the average). Wasm has no preemptive
        // timeout primitive here; legs keep their own client timeouts.
        #[cfg(not(target_arch = "wasm32"))]
        let outcome = {
            let t0 = std::time::Instant::now();
            let r = tokio::time::timeout(
                crate::streaming::ranking::PROVIDER_BUDGET,
                provider.resolve(&query),
            )
            .await;
            let elapsed_ms = t0.elapsed().as_millis() as u64;
            match r {
                Ok(res) => {
                    let ok = matches!(res, Resolution::Success { .. });
                    crate::streaming::ranking::record(provider.name(), ok, ok.then_some(elapsed_ms));
                    res
                }
                Err(_) => {
                    tracing::warn!(
                        "provider {} exceeded leg budget, trying next",
                        provider.name()
                    );
                    crate::streaming::ranking::record(provider.name(), false, None);
                    continue;
                }
            }
        };
        #[cfg(target_arch = "wasm32")]
        let outcome = {
            let r = provider.resolve(&query).await;
            crate::streaming::ranking::record(
                provider.name(),
                matches!(r, Resolution::Success { .. }),
                None,
            );
            r
        };
        match outcome {
            Resolution::Success {
                url,
                format,
                quality,
            } => {
                tracing::info!(
                    "resolved {track_id} via {} → {format:?} {quality:?}",
                    provider.name()
                );
                // Cache the result (quality rides along — cache hits must
                // report the real tier, never assumed lossless). Preview
                // providers opt out (signed minutes-out URLs would die in
                // the 50-min cache and serve dead signatures).
                if provider.cacheable() {
                    cache::put(
                        track_id,
                        provider.name(),
                        &url,
                        &format.to_string(),
                        &quality.to_string(),
                    );
                }
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
                // Deterministic miss: remember it (TTL'd) so replays skip
                // this provider for this track. Errors/cooldowns never
                // land here — only content verdicts.
                #[cfg(not(target_arch = "wasm32"))]
                if let Ok(mut n) = negative().lock() {
                    n.record(track_id, provider.name(), std::time::Instant::now());
                }
                continue;
            }
            Resolution::Error(e) => {
                tracing::warn!("provider {} error: {e}", provider.name());
                continue;
            }
        }
    }

    // Miss-path ISRC enrichment (Phase E): the name-based chain found
    // nothing. If an ISRC consumer is configured and we have no ISRC yet,
    // look one up (cached, rate-gated) and retry ONLY that consumer — other
    // providers can't use an ISRC, so a full second pass would just burn
    // timeouts for nothing.
    if query.isrc.is_none() && providers::qobuz::is_configured() {
        if let Some(isrc) =
            super::isrc::lookup_isrc(&query.title, &query.artist, query.duration_ms).await
        {
            tracing::info!("enriched {track_id} with ISRC, retrying ISRC consumers");
            let mut enriched = query.clone();
            enriched.isrc = Some(isrc);
            // Reuse the chain member (shared client, cooldown + credential
            // state) — never a second throwaway instance.
            let qobuz = chain.iter().find(|p| p.name() == "qobuz");
            let Some(qobuz) = qobuz else {
                return Ok(None);
            };
            if let Resolution::Success {
                url,
                format,
                quality,
            } = qobuz.resolve(&enriched).await
            {
                tracing::info!("resolved {track_id} via qobuz (ISRC) → {format:?} {quality:?}");
                cache::put(
                    track_id,
                    qobuz.name(),
                    &url,
                    &format.to_string(),
                    &quality.to_string(),
                );
                return Ok(Some(ResolvedStream {
                    url,
                    format,
                    quality,
                    provider: qobuz.name().to_string(),
                }));
            }
        }
    }

    Ok(None) // No provider could resolve this track.
}

/// Build a `TrackQuery` from Spotify track metadata.
fn build_query(track: &crate::spotify::models::Track) -> TrackQuery {
    // All artists, not just the first: "A feat. B" searched as "A" alone
    // misses or misresolves on every provider.
    let artist = track
        .artists
        .iter()
        .map(|a| a.name.clone())
        .filter(|n| !n.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    let album = track.album.name.clone();
    // No album-detail ISRC source exists yet: the miss path enriches via
    // MusicBrainz (see above) instead of forcing it up front.
    TrackQuery {
        spotify_id: track.id.clone(),
        isrc: None,
        title: track.name.clone(),
        artist,
        album: if album.is_empty() { None } else { Some(album) },
        duration_ms: track.duration_ms,
        max_quality: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(not(target_arch = "wasm32"))]
    use std::time::{Duration, Instant};

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn negative_cache_skips_until_expiry() {
        let mut c = NegativeCache::new(Duration::from_secs(3600));
        let now = Instant::now();
        assert!(!c.skip("t1", "invidious", now));
        c.record("t1", "invidious", now);
        assert!(c.skip("t1", "invidious", now));
        // Other tracks/providers unaffected.
        assert!(!c.skip("t2", "invidious", now));
        assert!(!c.skip("t1", "youtube", now));
        // Expired entries stop skipping.
        assert!(!c.skip("t1", "invidious", now + Duration::from_secs(3601)));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn negative_cache_zero_ttl_never_skips() {
        let mut c = NegativeCache::new(Duration::ZERO);
        let now = Instant::now();
        c.record("t1", "invidious", now);
        assert!(!c.skip("t1", "invidious", now));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn negative_cache_ignores_empty_track_ids() {
        let mut c = NegativeCache::new(Duration::from_secs(3600));
        let now = Instant::now();
        c.record("", "invidious", now);
        assert!(!c.skip("", "invidious", now));
        assert!(c.map.is_empty());
    }
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
            added_at: String::new(),
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

