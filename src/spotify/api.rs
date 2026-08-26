use crate::app_error::AppError;
use crate::spotify::client;
use crate::spotify::models::*;
use crate::spotify::session;
use crate::spotify::store;

const API_BASE: &str = "https://api.spotify.com/v1";

/// GET a cacheable endpoint through the store tier: memory TTL hits,
/// single-flighted fetches (prefetch joins page mounts) and disk
/// stale-while-revalidate snapshots.
async fn cached_get_json(url: &str) -> Result<serde_json::Value, AppError> {
    let body = store::Store::global()
        .clone()
        .resolve(
            url.to_owned(),
            true,
            |url| async move { pipeline_load(&url).await },
        )
        .await?;
    serde_json::from_slice(&body).map_err(|e| AppError::Spotify(e.to_string()))
}

/// GET an uncachable endpoint directly (search results and other live data).
/// Still capped-wait on 429 and refresh-retry on 401; never cached.
async fn live_get_json(url: &str) -> Result<serde_json::Value, AppError> {
    let token = session::ensure_token().await?;
    match request_once(url, &token).await? {
        ResponseOutcome::Success(body) => serde_json::from_str(&body)
            .map_err(|e| AppError::Spotify(e.to_string())),
        ResponseOutcome::Throttled(secs) => {
            tokio::time::sleep(std::time::Duration::from_secs(secs.min(5))).await;
            let fresh = session::ensure_token().await?;
            let body = request_after_backoff(url, &fresh).await?;
            serde_json::from_str(&body).map_err(|e| AppError::Spotify(e.to_string()))
        }
        ResponseOutcome::Unauthorized => {
            let fresh = session::ensure_token().await?;
            match request_once(url, &fresh).await? {
                ResponseOutcome::Success(body) => {
                    serde_json::from_str(&body)
                        .map_err(|e| AppError::Spotify(e.to_string()))
                }
                ResponseOutcome::Unauthorized => {
                    crate::auth::logout();
                    Err(AppError::Auth("session revoked".into()))
                }
                ResponseOutcome::Forbidden(err) => Err(err),
                ResponseOutcome::Throttled(_) => Err(AppError::RateLimited),
                ResponseOutcome::ApiError(status, msg) => {
                    Err(AppError::Spotify(format!("{status}: {msg}")))
                }
            }
        }
        ResponseOutcome::Forbidden(err) => Err(err),
        ResponseOutcome::ApiError(status, msg) => {
            Err(AppError::Spotify(format!("{status}: {msg}")))
        }
    }
}

/// The loader the store runs on a miss: token lifecycle plus the
/// 401-refresh / capped-429 retry pipeline (formerly `api_get_json`).
async fn pipeline_load(url: &str) -> Result<Vec<u8>, AppError> {
    let token = session::ensure_token().await?;
    match request_once(url, &token).await? {
        ResponseOutcome::Success(body) => Ok(body.into_bytes()),
        ResponseOutcome::Unauthorized => {
            // Token went stale — refresh exactly once, then retry.
            let fresh = session::ensure_token().await?;
            match request_once(url, &fresh).await? {
                ResponseOutcome::Success(body) => Ok(body.into_bytes()),
                ResponseOutcome::Throttled(secs) => {
                    tokio::time::sleep(std::time::Duration::from_secs(secs.min(5))).await;
                    let fresh2 = session::ensure_token().await?;
                    Ok(request_after_backoff(url, &fresh2).await?.into_bytes())
                }
                ResponseOutcome::Unauthorized => {
                    crate::auth::logout();
                    Err(AppError::Auth("session revoked".into()))
                }
                ResponseOutcome::Forbidden(err) => Err(err),
                ResponseOutcome::ApiError(status, msg) => {
                    Err(AppError::Spotify(format!("{status}: {msg}")))
                }
            }
        }
        ResponseOutcome::Throttled(secs) => {
            // One short capped wait, then surface a clear error — pages have
            // their own retry timers (never an endless spinner).
            tokio::time::sleep(std::time::Duration::from_secs(secs.min(5))).await;
            let fresh = session::ensure_token().await?;
            Ok(request_after_backoff(url, &fresh).await?.into_bytes())
        }
        ResponseOutcome::Forbidden(err) => Err(err),
        ResponseOutcome::ApiError(status, msg) => {
            Err(AppError::Spotify(format!("{status}: {msg}")))
        }
    }
}

enum ResponseOutcome {
    Success(String),
    Unauthorized,
    Throttled(u64),
    /// 403 — Spotify gate. `/me/player` is Premium-only; anything else is a
    /// general access denial (dead session, geo-block, …).
    Forbidden(AppError),
    ApiError(u16, String),
}

async fn request_once(
    url: &str,
    token: &str,
) -> Result<ResponseOutcome, AppError> {
    let resp = client::filtered_get_auth(url, token).await?;
    let outcome = classify(resp, url).await;
    match &outcome {
        ResponseOutcome::Success(body) => tracing::info!("api: {url} -> 200 ({} bytes)", body.len()),
        ResponseOutcome::Unauthorized => tracing::info!("api: {url} -> 401"),
        ResponseOutcome::Throttled(secs) => tracing::info!("api: {url} -> 429 (retry-after {secs}s)"),
        ResponseOutcome::Forbidden(err) => tracing::info!("api: {url} -> 403 ({err})"),
        ResponseOutcome::ApiError(status, msg) => {
            tracing::info!("api: {url} -> {status} ({})", &msg[..msg.len().min(80)])
        }
    }
    Ok(outcome)
}

async fn request_after_backoff(
    url: &str,
    token: &str,
) -> Result<String, AppError> {
    let resp = client::filtered_get_auth(url, token).await?;
    match classify(resp, url).await {
        ResponseOutcome::Success(body) => Ok(body),
        ResponseOutcome::Unauthorized => {
            crate::auth::logout();
            Err(AppError::Auth("session revoked".into()))
        }
        ResponseOutcome::Forbidden(err) => Err(err),
        ResponseOutcome::Throttled(secs) => {
            tokio::time::sleep(std::time::Duration::from_secs(secs.min(5))).await;
            let resp2 = client::filtered_get_auth(url, token).await?;
            match classify(resp2, url).await {
                ResponseOutcome::Success(body) => Ok(body),
                ResponseOutcome::Unauthorized => {
                    crate::auth::logout();
                    Err(AppError::Auth("session revoked".into()))
                }
                ResponseOutcome::Forbidden(err) => Err(err),
                ResponseOutcome::ApiError(status, msg) => {
                    Err(AppError::Spotify(format!("{status}: {msg}")))
                }
                ResponseOutcome::Throttled(_) => Err(AppError::RateLimited),
            }
        }
        ResponseOutcome::ApiError(status, msg) => {
            Err(AppError::Spotify(format!("{status}: {msg}")))
        }
    }
}

async fn classify(resp: reqwest::Response, url: &str) -> ResponseOutcome {
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return ResponseOutcome::Unauthorized;
    }
    if status == reqwest::StatusCode::FORBIDDEN {
        let body = resp.text().await.unwrap_or_default();
        tracing::warn!("api: 403 for {url}: {body}");
        let err = if url.contains("/me/player") {
            AppError::PremiumRequired("Playback requires Spotify Premium".into())
        } else {
            AppError::Forbidden("Access denied (check account or endpoint)".into())
        };
        return ResponseOutcome::Forbidden(err);
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
) -> Result<Paged<T>, AppError>
where
    T: serde::de::DeserializeOwned + std::default::Default,
{
    let url = format!("{API_BASE}{path}?limit={limit}&offset={offset}");
    let value = cached_get_json(&url).await?;
    serde_json::from_value(value).map_err(|e| AppError::Spotify(e.to_string()))
}

/// GET a single object by id.
async fn get_object<T>(path: &str) -> Result<T, AppError>
where
    T: serde::de::DeserializeOwned + std::default::Default,
{
    let url = format!("{API_BASE}{path}");
    let value = cached_get_json(&url).await?;
    serde_json::from_value(value).map_err(|e| AppError::Spotify(e.to_string()))
}

// ── endpoints ────────────────────────────────────────────────────────────────

pub async fn get_current_user_profile() -> Result<UserProfile, AppError> {
    let token = session::ensure_token().await?;
    let url = format!("{API_BASE}/me");
    let resp = client::filtered_get_auth(&url, &token).await?;
    resp.error_for_status()
        .map_err(AppError::from)?
        .json()
        .await
        .map_err(AppError::from)
}

pub async fn get_home() -> Result<HomeData, AppError> {
    tracing::info!("api: get_home start -- fanning out");
    let rec = tokio::spawn(async move { get_recommendations(&[]).await.unwrap_or_default() });
    let (featured, new_releases) = tokio::try_join!(get_featured_playlists(), get_new_releases())?;
    let recommended = rec.await.unwrap_or_default();
    tracing::info!("api: get_home done -- featured={} new_releases={} recommended={}", featured.len(), new_releases.len(), recommended.len());
    Ok(HomeData { featured, new_releases, recommended })
}
pub async fn search(
    q: &str,
    types: &[&str],
    limit: u32,
) -> Result<SearchResults, AppError> {
    let types = types.join(",");
    let encoded: String = url::form_urlencoded::byte_serialize(q.as_bytes()).collect();
    let url = format!("{API_BASE}/search?q={encoded}&type={types}&limit={limit}&market=from_token");
    let value = live_get_json(&url).await?;
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
    get_object(&format!("/albums/{id}")).await
}

pub async fn get_artist(id: &str) -> Result<Artist, AppError> {
    get_object(&format!("/artists/{id}")).await
}

pub async fn get_artist_top_tracks(id: &str) -> Result<Vec<Track>, AppError> {
    let url = format!("{API_BASE}/artists/{id}/top-tracks?market=from_token");
    let value = cached_get_json(&url).await?;
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
    let value = cached_get_json(&url).await?;
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
    fetch_page::<Playlist>("/me/playlists", 50, 0)
        .await
        .map(|page| page.items)
}

pub async fn get_user_saved_tracks(limit: u32, offset: u32) -> Result<Paged<crate::spotify::models::SavedTrack>, AppError> {
    fetch_page::<crate::spotify::models::SavedTrack>("/me/tracks", limit, offset).await
}

/// An artist's albums ("discography"), newest-first per Spotify.
pub async fn get_artist_albums(id: &str, limit: u32) -> Result<Vec<Album>, AppError> {
    let url = format!(
        "{API_BASE}/artists/{id}/albums?limit={limit}&market=from_token&include_groups=album,single"
    );
    let value = cached_get_json(&url).await?;
    let payload = value
        .get("items")
        .cloned()
        .unwrap_or(serde_json::Value::Array(vec![]));
    serde_json::from_value(payload).map_err(|e| AppError::Spotify(e.to_string()))
}

/// Artists related to the given one ("Fans also like").
pub async fn get_artist_related(id: &str) -> Result<Vec<Artist>, AppError> {
    let url = format!("{API_BASE}/artists/{id}/related-artists");
    let value = cached_get_json(&url).await?;
    let artists = value
        .get("artists")
        .cloned()
        .unwrap_or(serde_json::Value::Array(vec![]));
    serde_json::from_value(artists).map_err(|e| AppError::Spotify(e.to_string()))
}

pub async fn get_user_albums(limit: u32, offset: u32) -> Result<Vec<Album>, AppError> {
    fetch_page::<Album>("/me/albums", limit, offset)
        .await
        .map(|page| page.items)
}

pub async fn get_featured_playlists() -> Result<Vec<Playlist>, AppError> {
    let url = format!("{API_BASE}/browse/featured-playlists?limit=12&market=from_token");
    let value = cached_get_json(&url).await?;
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
    let value = cached_get_json(&url).await?;
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
    let value = cached_get_json(&url).await?;
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