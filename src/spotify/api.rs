use crate::app_error::AppError;
use crate::spotify::client;
use crate::spotify::gql;
use crate::spotify::models::*;
use crate::spotify::session;

const API_BASE: &str = "https://api.spotify.com/v1";

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
    tracing::info!("api: get_home start -- fanning out (GQL pathfinder)");
    let (playlists_res, liked_res) = tokio::join!(
        get_user_playlists(),
        get_user_saved_tracks(20, 0),
    );
    let playlists = playlists_res.unwrap_or_default();
    let liked_tracks: Vec<Track> = liked_res
        .map(|p| p.items.into_iter().filter_map(|st| st.track).collect())
        .unwrap_or_default();
    tracing::info!("api: get_home done -- playlists={} liked={}", playlists.len(), liked_tracks.len());
    Ok(HomeData { playlists, liked_tracks })
}
pub async fn search(
    q: &str,
    _types: &[&str],
    limit: u32,
) -> Result<SearchResults, AppError> {
    // Full-text search goes through the internal GraphQL API (pathfinder), not
    // the hard-rate-limited `/v1/search`.
    gql::gql_search(q, limit).await
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
    // Album detail + tracks go through the internal GraphQL API (pathfinder),
    // not the hard-rate-limited `/v1/albums/{id}`.
    gql::gql_album(id).await
}

/// Artist page: hero + discography + popular tracks + related artists.
#[derive(Debug, Clone, Default)]
pub struct ArtistPage {
    pub artist: Artist,
    pub albums: Vec<Album>,
    pub top_tracks: Vec<Track>,
    pub related: Vec<Artist>,
}

/// Full artist page via a single pathfinder query (plus one for related
/// artists), replacing four `/v1/artists/{id}` calls that hard 429.
pub async fn get_artist_page(id: &str) -> Result<ArtistPage, AppError> {
    let (artist, albums, top_tracks, related) = gql::gql_artist_page(id).await?;
    Ok(ArtistPage {
        artist,
        albums,
        top_tracks,
        related,
    })
}

pub async fn get_playlist(id: &str) -> Result<Playlist, AppError> {
    // Playlist detail goes through the internal GraphQL API (pathfinder), not
    // the hard-rate-limited `/v1`. Returns playlist metadata + its tracks.
    gql::gql_playlist(id).await
}

pub async fn get_user_playlists() -> Result<Vec<Playlist>, AppError> {
    // User-owned data goes through Spotify's internal GraphQL API, which accepts
    // our web-player token and is not subject to the `/v1` hard rate limit.
    gql::gql_user_playlists(50, 0).await
}

pub async fn get_user_saved_tracks(limit: u32, offset: u32) -> Result<Paged<SavedTrack>, AppError> {
    let tracks = gql::gql_user_liked_tracks(limit, offset).await?;
    let total = tracks.len() as u32;
    Ok(Paged {
        items: tracks
            .into_iter()
            .map(|t| SavedTrack {
                added_at: String::new(),
                track: Some(t),
            })
            .collect(),
        total,
        limit,
        offset,
        next: None,
        previous: None,
    })
}

pub async fn get_user_albums(limit: u32, offset: u32) -> Result<Vec<Album>, AppError> {
    // Saved albums go through the internal GraphQL API (pathfinder); the old
    // `/me/albums` endpoint is hard-rate-limited to 429.
    gql::gql_user_albums(limit, offset).await
}

pub async fn get_album_tracks(id: &str) -> Result<Vec<Track>, AppError> {
    let album = get_album(id).await?;
    Ok(album
        .tracks
        .map(|page| page.items)
        .unwrap_or_default())
}

/// Fetch a single track by ID.
pub async fn get_track(id: &str) -> Result<Track, AppError> {
    // Single-track metadata goes through the internal GraphQL API (pathfinder),
    // not the hard-rate-limited `/v1/tracks/{id}`.
    gql::gql_track(id)
        .await?
        .ok_or_else(|| AppError::Spotify(format!("track {id} not found via GQL search")))
}