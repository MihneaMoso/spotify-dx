use crate::app_error::AppError;
use crate::spotify::cache;
use crate::spotify::client;
use crate::spotify::models::*;
use crate::spotify::session;

const API_BASE: &str = "https://api.spotify.com/v1";

/// GET an endpoint, honoring the ad-filter, in-memory TTL cache, 401 refresh and
/// 429 Retry-After backoff. Falls back to the on-disk cache on network errors.
async fn api_get_json(url: &str, cacheable: bool) -> Result<serde_json::Value, AppError> {
    let token = session::ensure_token().await?;

    if let Some(cached) = cache::get(url) {
        return serde_json::from_slice(&cached).map_err(|e| AppError::Spotify(e.to_string()));
    }

    let (body, _fresh) = match request_once(url, &token).await? {
        ResponseOutcome::Success(body) => (body, true),
        ResponseOutcome::Unauthorized => {
            // Token went stale — refresh exactly once, then retry.
            let fresh = session::ensure_token().await?;
            match request_once(url, &fresh).await? {
                ResponseOutcome::Success(body) => (body, true),
                ResponseOutcome::Throttled(secs) => {
                    tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
                    let fresh2 = session::ensure_token().await?;
                    let body = request_after_backoff(url, &fresh2).await?;
                    (body, true)
                }
                ResponseOutcome::Unauthorized => {
                    crate::auth::logout();
                    return Err(AppError::Auth("session revoked".into()));
                }
                ResponseOutcome::ApiError(status, msg) => {
                    return Err(AppError::Spotify(format!("{status}: {msg}")))
                }
            }
        }
        ResponseOutcome::Throttled(secs) => {
            tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
            let body = request_after_backoff(url, &token).await?;
            (body, true)
        }
        ResponseOutcome::ApiError(status, msg) => {
            return Err(AppError::Spotify(format!("{status}: {msg}")));
        }
    };

    let value: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| AppError::Spotify(e.to_string()))?;
    if cacheable {
        cache::put(url, body.into_bytes());
    }
    Ok(value)
}

enum ResponseOutcome {
    Success(String),
    Unauthorized,
    Throttled(u64),
    ApiError(u16, String),
}

async fn request_once(
    url: &str,
    token: &str,
) -> Result<ResponseOutcome, AppError> {
    let resp = client::filtered_get_auth(url, token).await?;
    Ok(classify(resp).await)
}

async fn request_after_backoff(
    url: &str,
    token: &str,
) -> Result<String, AppError> {
    let resp = client::filtered_get_auth(url, token).await?;
    match classify(resp).await {
        ResponseOutcome::Success(body) => Ok(body),
        ResponseOutcome::Unauthorized => {
            crate::auth::logout();
            Err(AppError::Auth("session revoked".into()))
        }
        ResponseOutcome::Throttled(secs) => {
            tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
            let resp2 = client::filtered_get_auth(url, token).await?;
            match classify(resp2).await {
                ResponseOutcome::Success(body) => Ok(body),
                ResponseOutcome::Unauthorized => {
                    crate::auth::logout();
                    Err(AppError::Auth("session revoked".into()))
                }
                ResponseOutcome::ApiError(status, msg) => {
                    Err(AppError::Spotify(format!("{status}: {msg}")))
                }
                ResponseOutcome::Throttled(_) => {
                    Err(AppError::Spotify("rate limited after retry".into()))
                }
            }
        }
        ResponseOutcome::ApiError(status, msg) => {
            Err(AppError::Spotify(format!("{status}: {msg}")))
        }
    }
}

async fn classify(resp: reqwest::Response) -> ResponseOutcome {
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return ResponseOutcome::Unauthorized;
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        let secs = resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(5);
        return ResponseOutcome::Throttled(secs);
    }
    if status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return ResponseOutcome::Success(body);
    }
    let body = resp.text().await.unwrap_or_default();
    ResponseOutcome::ApiError(status.as_u16(), body)
}

/// Drop-in page-fetch that also works for browsing without hitting the network.
async fn fetch_page<T>(
    path: &str,
    limit: u32,
    offset: u32,
    cacheable: bool,
) -> Result<Paged<T>, AppError>
where
    T: serde::de::DeserializeOwned + std::default::Default,
{
    let url = format!("{API_BASE}{path}?limit={limit}&offset={offset}");
    let value = api_get_json(&url, cacheable).await?;
    serde_json::from_value(value).map_err(|e| AppError::Spotify(e.to_string()))
}

/// GET a single object by id.
async fn get_object<T>(path: &str, cacheable: bool) -> Result<T, AppError>
where
    T: serde::de::DeserializeOwned + std::default::Default,
{
    let url = format!("{API_BASE}{path}");
    let value = api_get_json(&url, cacheable).await?;
    serde_json::from_value(value).map_err(|e| AppError::Spotify(e.to_string()))
}

// ── endpoints ────────────────────────────────────────────────────────────────

pub async fn get_current_user_profile(
    access_token: &str,
) -> Result<UserProfile, AppError> {
    let url = format!("{API_BASE}/me");
    let resp = client::filtered_get_auth(&url, access_token).await?;
    resp.error_for_status()
        .map_err(AppError::from)?
        .json()
        .await
        .map_err(AppError::from)
}

pub async fn get_home() -> Result<HomeData, AppError> {
    let featured = get_featured_playlists().await?;
    let new_releases = get_new_releases().await?;
    let recommended = get_recommendations(&[]).await.unwrap_or_default();
    Ok(HomeData {
        featured,
        new_releases,
        recommended,
    })
}

pub async fn search(
    q: &str,
    types: &[&str],
    limit: u32,
) -> Result<SearchResults, AppError> {
    let types = types.join(",");
    let encoded: String = url::form_urlencoded::byte_serialize(q.as_bytes()).collect();
    let url = format!("{API_BASE}/search?q={encoded}&type={types}&limit={limit}&market=from_token");
    let value = api_get_json(&url, false).await?;
    serde_json::from_value(value).map_err(|e| AppError::Spotify(e.to_string()))
}

pub async fn search_tracks(q: &str, limit: u32) -> Result<Vec<Track>, AppError> {
    search(q, &["track"], limit)
        .await
        .map(|results| {
            results
                .tracks
                .map(|page| page.items)
                .unwrap_or_default()
        })
}

pub async fn get_album(id: &str) -> Result<Album, AppError> {
    get_object(&format!("/albums/{id}"), true).await
}

pub async fn get_artist(id: &str) -> Result<Artist, AppError> {
    get_object(&format!("/artists/{id}"), true).await
}

pub async fn get_artist_top_tracks(id: &str) -> Result<Vec<Track>, AppError> {
    let url = format!("{API_BASE}/artists/{id}/top-tracks?market=from_token");
    let value = api_get_json(&url, true).await?;
    let tracks = value
        .get("tracks")
        .and_then(|t| t.as_array())
        .cloned()
        .unwrap_or_default();
    serde_json::from_value(serde_json::Value::Array(tracks))
        .map_err(|e| AppError::Spotify(e.to_string()))
}

pub async fn get_playlist(id: &str) -> Result<Playlist, AppError> {
    let url = format!("{API_BASE}/playlists/{id}?market=from_token");
    let value = api_get_json(&url, true).await?;
    let mut playlist: Playlist = serde_json::from_value(value.clone())
        .map_err(|e| AppError::Spotify(e.to_string()))?;

    // Spotify nests each playlist entry under `items[].track`; flatten it back
    // into the plain track list our model expects.
    if let Some(items) = value
        .get("tracks")
        .and_then(|t| t.get("items"))
        .and_then(|a| a.as_array())
    {
        let tracks: Vec<Track> = items
            .iter()
            .filter_map(|item| item.get("track").filter(|t| !t.is_null()))
            .filter_map(|track| serde_json::from_value::<Track>(track.clone()).ok())
            .collect();
        playlist.tracks.items = tracks;
        playlist.tracks.total = value
            .get("tracks")
            .and_then(|t| t.get("total"))
            .and_then(|n| n.as_u64())
            .unwrap_or(playlist.tracks.items.len() as u64) as u32;
    }
    Ok(playlist)
}

pub async fn get_user_playlists() -> Result<Vec<Playlist>, AppError> {
    fetch_page::<Playlist>("/me/playlists", 50, 0, true)
        .await
        .map(|page| page.items)
}

pub async fn get_user_saved_tracks(limit: u32, offset: u32) -> Result<Paged<Track>, AppError> {
    fetch_page::<Track>("/me/tracks", limit, offset, true).await
}

pub async fn get_user_albums(limit: u32, offset: u32) -> Result<Vec<Album>, AppError> {
    fetch_page::<Album>("/me/albums", limit, offset, true)
        .await
        .map(|page| page.items)
}

pub async fn get_featured_playlists() -> Result<Vec<Playlist>, AppError> {
    let url = format!("{API_BASE}/browse/featured-playlists?limit=12&market=from_token");
    let value = api_get_json(&url, true).await?;
    let payload = value
        .get("playlists")
        .cloned()
        .ok_or_else(|| AppError::Spotify("playlists key missing".into()))?;
    let page: Paged<Playlist> =
        serde_json::from_value(payload).map_err(|e| AppError::Spotify(e.to_string()))?;
    Ok(page.items)
}

pub async fn get_new_releases() -> Result<Vec<Album>, AppError> {
    let url = format!("{API_BASE}/browse/new-releases?limit=12&market=from_token");
    let value = api_get_json(&url, true).await?;
    let payload = value
        .get("albums")
        .cloned()
        .ok_or_else(|| AppError::Spotify("albums key missing".into()))?;
    let page: Paged<Album> =
        serde_json::from_value(payload).map_err(|e| AppError::Spotify(e.to_string()))?;
    Ok(page.items)
}

pub async fn get_recommendations(seed_tracks: &[&str]) -> Result<Vec<Track>, AppError> {
    let seeds = if seed_tracks.is_empty() {
        "4NHQUGzhtTLFpeF5Z4S3zy,3n3Ppam7vgaVa1iaRUc9Lp".to_owned()
    } else {
        seed_tracks.join(",")
    };
    let url = format!("{API_BASE}/recommendations?limit=20&market=from_token&seed_tracks={seeds}");
    let value = api_get_json(&url, true).await?;
    let tracks = value
        .get("tracks")
        .cloned()
        .unwrap_or(serde_json::Value::Array(vec![]));
    serde_json::from_value(tracks).map_err(|e| AppError::Spotify(e.to_string()))
}

pub async fn get_album_tracks(id: &str) -> Result<Vec<Track>, AppError> {
    let album = get_album(id).await?;
    Ok(album
        .tracks
        .map(|page| page.items)
        .unwrap_or_default())
}