//! Lyrics via LRCLIB (free, keyless, synced + plain).
//!
//! Flow per track: exact `/api/get` (artist/title/album/duration) → on 404,
//! `/api/search` with duration match (±10s, synced preferred). Misses are
//! normal (many tracks have no lyrics) and surface as `found: false`, never
//! errors. Responses cache in the core store (lyrics are immutable —
//! stale-while-revalidate is ideal) and single-flight coalesces repeat taps.

#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

/// Duration match tolerance for search fallback (LRCLIB reports seconds).
const SEARCH_TOLERANCE_SECS: f64 = 10.0;

/// Lyrics payload (serializable for the store + bridge).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct LyricsResult {
    pub found: bool,
    pub synced: String,
    pub plain: String,
    pub instrumental: bool,
}

/// Fetch lyrics for a track (store-cached, single-flight).
pub async fn fetch_lyrics(
    artist: &str,
    title: &str,
    album: &str,
    duration_ms: u64,
) -> Result<LyricsResult, crate::app_error::AppError> {
    let key = format!(
        "lyrics:{}:{}",
        artist.trim().to_lowercase(),
        title.trim().to_lowercase()
    );
    // Owned copies: the store loader must be 'static.
    let artist = artist.to_string();
    let title = title.to_string();
    let album = album.to_string();
    // The store's single-flight future is Send-bound (native threads); wasm
    // runs single-threaded, so it fetches direct (platform storage seam
    // owns wasm persistence instead).
    #[cfg(not(target_arch = "wasm32"))]
    {
        let bytes = crate::spotify::store::Store::global()
            .clone()
            .resolve(key.clone(), true, move |_| async move {
                let result = fetch_live(&artist, &title, &album, duration_ms).await?;
                serde_json::to_vec(&result).map_err(|e| {
                    crate::app_error::AppError::Spotify(format!("lyrics encode: {e}"))
                })
            })
            .await?;
        serde_json::from_slice::<LyricsResult>(&bytes).map_err(|e| {
            crate::app_error::AppError::Spotify(format!("lyrics decode: {e}"))
        })
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = key;
        fetch_live(&artist, &title, &album, duration_ms).await
    }
}

async fn client() -> Result<reqwest::Client, crate::app_error::AppError> {
    let builder = reqwest::Client::builder();
    #[cfg(not(target_arch = "wasm32"))]
    let builder = builder.timeout(Duration::from_secs(8));
    builder
        .build()
        .map_err(|e| crate::app_error::AppError::Spotify(format!("lyrics client: {e}")))
}

/// Exact match; 404 means "try search", not failure.
async fn fetch_live(
    artist: &str,
    title: &str,
    album: &str,
    duration_ms: u64,
) -> Result<LyricsResult, crate::app_error::AppError> {
    let client = client().await?;
    let mut url = format!(
        "https://lrclib.net/api/get?artist_name={}&track_name={}",
        urlencode(artist),
        urlencode(title)
    );
    if !album.is_empty() {
        url.push_str(&format!("&album_name={}", urlencode(album)));
    }
    if duration_ms > 0 {
        url.push_str(&format!("&duration={}", duration_ms / 1000));
    }
    let resp = client.get(&url).send().await.map_err(other_err)?;
    if resp.status().as_u16() == 404 {
        return search_fallback(&client, artist, title, duration_ms).await;
    }
    if !resp.status().is_success() {
        return Err(other_err(format!("lyrics status {}", resp.status())));
    }
    let val: serde_json::Value = resp.json().await.map_err(other_err)?;
    Ok(from_api_object(&val))
}

/// Search fallback: first duration-acceptable hit with lyrics (synced
/// preferred). Miss → `found: false` (normal, not an error).
async fn search_fallback(
    client: &reqwest::Client,
    artist: &str,
    title: &str,
    duration_ms: u64,
) -> Result<LyricsResult, crate::app_error::AppError> {
    let url = format!(
        "https://lrclib.net/api/search?q={}+{}",
        urlencode(artist),
        urlencode(title)
    );
    let resp = client.get(&url).send().await.map_err(other_err)?;
    if resp.status().as_u16() == 404 {
        return Ok(LyricsResult::default());
    }
    if !resp.status().is_success() {
        return Err(other_err(format!("lyrics search status {}", resp.status())));
    }
    let val: serde_json::Value = resp.json().await.map_err(other_err)?;
    let items = match val.as_array() {
        Some(i) => i,
        None => return Ok(LyricsResult::default()),
    };
    // Pass 1: duration-matched with synced lines. Pass 2: any lyrics.
    for synced_only in [true, false] {
        for item in items {
            if duration_ms != 0 {
                if let Some(d) = item.get("duration").and_then(|d| d.as_f64()) {
                    if (d * 1000.0 - duration_ms as f64).abs() > SEARCH_TOLERANCE_SECS * 1000.0 {
                        continue;
                    }
                }
            }
            let r = from_api_object(item);
            if !r.found {
                continue;
            }
            if synced_only && r.synced.is_empty() {
                continue;
            }
            return Ok(r);
        }
    }
    Ok(LyricsResult::default())
}

/// Map one LRCLIB track object onto the result. Pure (unit-tested).
fn from_api_object(v: &serde_json::Value) -> LyricsResult {
    let synced = v.get("syncedLyrics").and_then(|s| s.as_str()).unwrap_or("");
    let plain = v.get("plainLyrics").and_then(|s| s.as_str()).unwrap_or("");
    let instrumental = v.get("instrumental").and_then(|b| b.as_bool()).unwrap_or(false);
    LyricsResult {
        found: instrumental || !synced.is_empty() || !plain.is_empty(),
        synced: synced.to_string(),
        plain: plain.to_string(),
        instrumental,
    }
}

fn other_err(e: impl std::fmt::Display) -> crate::app_error::AppError {
    crate::app_error::AppError::Spotify(format!("lyrics: {e}"))
}

/// Minimal percent-encoding for query params (mirrors providers).
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_api_object_reads_shapes() {
        let v = serde_json::json!({
            "syncedLyrics": "[00:01.00] hi",
            "plainLyrics": "hi",
            "instrumental": false
        });
        let r = from_api_object(&v);
        assert!(r.found);
        assert_eq!(r.synced, "[00:01.00] hi");
        assert!(!r.instrumental);

        let inst = serde_json::json!({ "instrumental": true });
        let r = from_api_object(&inst);
        assert!(r.found && r.instrumental && r.synced.is_empty());

        assert!(!from_api_object(&serde_json::json!({})).found);
    }

    #[test]
    fn roundtrip_json_for_store() {
        let r = LyricsResult {
            found: true,
            synced: "[00:01.00] hi".into(),
            plain: "hi".into(),
            instrumental: false,
        };
        let bytes = serde_json::to_vec(&r).unwrap();
        assert_eq!(serde_json::from_slice::<LyricsResult>(&bytes).unwrap(), r);
    }
}
