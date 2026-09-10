//! MusicBrainz ISRC enrichment (miss-path only).
//!
//! When the name-based provider chain misses, a recording lookup can still
//! yield the ISRC, unlocking ISRC-capable providers (Qobuz). Runs ONLY on
//! the miss path and only when an ISRC consumer is actually configured —
//! never on the happy path, never speculatively.
//!
//! Etiquette: MusicBrainz requires a User-Agent and ≤1 req/s. Both are
//! enforced here (shared call gate + process cache, so repeat misses for
//! the same track cost nothing).

use std::collections::HashMap;
use std::sync::Mutex;
#[cfg(not(target_arch = "wasm32"))]
use std::time::{Duration, Instant};

/// Minimum gap between MusicBrainz calls (their rate policy).
#[cfg(not(target_arch = "wasm32"))]
const CALL_GAP: Duration = Duration::from_millis(1100);
/// Cache cap: ISRCs are immutable, but the map must stay bounded.
const CACHE_CAP: usize = 512;
/// Length match tolerance (live/extended versions differ legitimately).
const LENGTH_TOLERANCE_MS: u64 = 15_000;

static CACHE: Mutex<Option<HashMap<String, Option<String>>>> = Mutex::new(None);
#[cfg(not(target_arch = "wasm32"))]
static LAST_CALL: Mutex<Option<Instant>> = Mutex::new(None);

fn cache_get(key: &str) -> Option<Option<String>> {
    CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|m| m.get(key).cloned())
}

fn cache_put(key: String, value: Option<String>) {
    if let Ok(mut guard) = CACHE.lock() {
        let map = guard.get_or_insert_with(HashMap::new);
        if map.len() >= CACHE_CAP {
            map.clear();
        }
        map.insert(key, value);
    }
}

/// Look up the ISRC for a track (miss-path enrichment).
/// Returns the first ISRC of the best-matching recording, if any.
pub async fn lookup_isrc(title: &str, artist: &str, duration_ms: u64) -> Option<String> {
    let key = format!("{title}\0{artist}");
    if let Some(cached) = cache_get(&key) {
        return cached;
    }
    let found = fetch_isrc(title, artist, duration_ms).await;
    cache_put(key, found.clone());
    found
}

async fn fetch_isrc(title: &str, artist: &str, duration_ms: u64) -> Option<String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        // Shared 1 req/s gate (MusicBrainz policy).
        let wait = LAST_CALL
            .lock()
            .ok()
            .and_then(|last| {
                last.as_ref()
                    .and_then(|t| CALL_GAP.checked_sub(t.elapsed()))
            })
            .unwrap_or(Duration::from_millis(0));
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
        if let Ok(mut last) = LAST_CALL.lock() {
            *last = Some(Instant::now());
        }
    }
    let query = format!(
        "recording:\"{}\" AND artist:\"{}\"",
        title.replace('"', ""),
        artist.replace('"', "")
    );
    let url = format!(
        "https://musicbrainz.org/ws/2/recording/?query={}&fmt=json&limit=5",
        urlencoding::encode(&query)
    );
    let resp = reqwest::Client::builder()
        .user_agent("SpotifyDX/0.1 (https://github.com/MihneaMoso/spotify-dx)")
        .build()
        .ok()?
        .get(&url)
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let val: serde_json::Value = resp.json().await.ok()?;
    pick_isrc(
        val.get("recordings").and_then(|r| r.as_array()),
        title,
        artist,
        duration_ms,
    )
}

/// Best-match recording's first ISRC: strict title, lenient artist
/// (featuring!), lenient length. Pure (unit-tested).
fn pick_isrc(
    recordings: Option<&Vec<serde_json::Value>>,
    title: &str,
    artist: &str,
    duration_ms: u64,
) -> Option<String> {
    let recordings = recordings?;
    let want_title = title.to_lowercase();
    let want_artist = artist.to_lowercase();
    for rec in recordings {
        let title_ok = rec
            .get("title")
            .and_then(|t| t.as_str())
            .map(|t| t.to_lowercase() == want_title)
            .unwrap_or(false);
        if !title_ok {
            continue;
        }
        let artist_ok = rec
            .get("artist-credit")
            .and_then(|c| c.as_array())
            .map(|credits| {
                credits.iter().any(|c| {
                    c.get("name")
                        .and_then(|n| n.as_str())
                        .map(|n| {
                            let n = n.to_lowercase();
                            n.contains(&want_artist) || want_artist.contains(&n)
                        })
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false);
        if !artist_ok {
            continue;
        }
        if duration_ms != 0 {
            if let Some(len) = rec.get("length").and_then(|l| l.as_u64()) {
                if duration_ms.abs_diff(len) > LENGTH_TOLERANCE_MS {
                    continue;
                }
            }
        }
        if let Some(isrc) = rec
            .get("isrcs")
            .and_then(|i| i.as_array())
            .and_then(|a| a.first())
            .and_then(|s| s.as_str())
        {
            if !isrc.is_empty() {
                return Some(isrc.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(title: &str, artist: &str, len: u64, isrcs: &[&str]) -> serde_json::Value {
        serde_json::json!({
            "title": title,
            "length": len,
            "artist-credit": [{ "name": artist }],
            "isrcs": isrcs,
        })
    }

    #[test]
    fn pick_isrc_matches_strict_title_lenient_artist() {
        let recs = vec![
            rec("Mercy", "Kanye West", 150_000, &[]),
            rec("Mercy", "Kanye West feat. Someone", 160_000, &["US-AAA-11-11111"]),
            rec("Mercy", "Other Artist", 160_000, &["US-BBB-22-22222"]),
        ];
        // Exact title + featuring artist + in-tolerance length wins.
        assert_eq!(
            pick_isrc(Some(&recs), "Mercy", "Kanye West", 165_000).as_deref(),
            Some("US-AAA-11-11111")
        );
        // Wrong title never matches.
        assert_eq!(pick_isrc(Some(&recs), "Mercyful", "Kanye West", 165_000), None);
        // Hour-long mismatch rejected.
        assert_eq!(pick_isrc(Some(&recs), "Mercy", "Kanye West", 3_600_000), None);
        // Unknown track length skips the length gate.
        assert_eq!(
            pick_isrc(Some(&recs), "Mercy", "Kanye West", 0).as_deref(),
            Some("US-AAA-11-11111")
        );
    }

    #[test]
    fn pick_isrc_empty_shapes() {
        assert_eq!(pick_isrc(None, "T", "A", 0), None);
        assert_eq!(pick_isrc(Some(&vec![]), "T", "A", 0), None);
    }
}
