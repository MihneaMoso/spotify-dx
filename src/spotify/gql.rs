use crate::app_error::AppError;
use crate::spotify::client;
use crate::spotify::session;
use serde_json::{json, Value};

/// Spotify's internal GraphQL persisted-query API. This is the endpoint the
/// web player (`open.spotify.com`) actually uses for the home feed, library,
/// playlists, search, and album/artist detail. It accepts the same web-player
/// bearer token we capture from `/api/token`, and (unlike `api.spotify.com/v1`)
/// is effectively free of the hard 429 rate limiting that plagues `/v1`.
const PATHFINDER: &str = "https://api-partner.spotify.com/pathfinder/v2/query";

/// Persisted-query SHA-256 hashes, matching what the current web player ships.
/// Spotify rotates these between app releases; if an operation returns
/// `PersistedQueryNotFound` (412), the hash is stale and must be refreshed.
#[allow(dead_code)] // some hashes are reserved for upcoming endpoints
mod hashes {
    pub const LIBRARY_V3: &str =
        "973e511ca44261fda7eebac8b653155e7caee3675abb4fb110cc1b8c78b091c3";
    pub const FETCH_LIBRARY_TRACKS: &str =
        "087278b20b743578a6262c2b0b4bcd20d879c503cc359a2285baf083ef944240";
    pub const FETCH_PLAYLIST: &str =
        "346811f856fb0b7e4f6c59f8ebea78dd081c6e2fb01b77c954b26259d5fc6763";
    pub const SEARCH_DESKTOP: &str =
        "4801118d4a100f756e833d33984436a3899cff359c532f8fd3aaf174b60b3b49";
    pub const GET_ALBUM: &str =
        "b9bfabef66ed756e5e13f68a942deb60bd4125ec1f1be8cc42769dc0259b4b10";
    pub const QUERY_ARTIST_OVERVIEW: &str =
        "ae0e2958a4ab645b35ca19ac04d0495ae12d9c5d7b7286217674801a9aab281a";
    pub const QUERY_ARTIST_RELATED: &str =
        "3d031d6cb22a2aa7c8d203d49b49df731f58b1e2799cc38d9876d58771aa66f3";
}

/// POST a persisted query to pathfinder and return the decoded `data` object.
async fn graphql_post(
    operation_name: &str,
    hash: &str,
    variables: Value,
) -> Result<Value, AppError> {
    let token = session::ensure_token().await?;

    let body = json!({
        "variables": variables,
        "operationName": operation_name,
        "extensions": {
            "persistedQuery": { "version": 1, "sha256Hash": hash }
        }
    });

    let resp = client::filtered_post_pathfinder(PATHFINDER, &token, body).await?;
    let status = resp.status();

    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        let secs = resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(5);
        tracing::info!("gql: {operation_name} -> 429 (retry-after {secs}s)");
        return Err(AppError::RateLimited);
    }
    if status == reqwest::StatusCode::UNAUTHORIZED {
        crate::auth::logout();
        return Err(AppError::Auth("session revoked".into()));
    }
    if status == reqwest::StatusCode::PRECONDITION_FAILED {
        // PersistedQueryNotFound — stale hash. Not auto-handled here.
        tracing::error!("gql: persisted query not found for {operation_name} (hash {hash})");
        return Err(AppError::Spotify(format!(
            "spotify GQL persisted-query hash mismatch for {operation_name}"
        )));
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(AppError::Spotify(format!(
            "gql {operation_name} -> {status}: {}",
            &body[..body.len().min(200)]
        )));
    }

    let value: Value = resp.json().await.map_err(|e| AppError::Spotify(e.to_string()))?;
    if let Some(errors) = value.get("errors").and_then(|e| e.as_array()) {
        if !errors.is_empty() {
            return Err(AppError::Spotify(format!(
                "gql {operation_name} errors: {errors:?}"
            )));
        }
    }
    value
        .get("data")
        .cloned()
        .ok_or_else(|| AppError::Spotify(format!("gql {operation_name}: missing data")))
}

fn str(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

fn uri_part(v: &Value) -> String {
    str(v).rsplit(':').next().unwrap_or_default().to_string()
}

fn parse_images(cover_art: &Value) -> Vec<crate::spotify::models::SpotifyImage> {
    cover_art
        .get("sources")
        .and_then(|s| s.as_array())
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|src| crate::spotify::models::SpotifyImage {
            url: str(&src["url"]),
            width: src["width"].as_u64().map(|w| w as u32),
            height: src["height"].as_u64().map(|h| h as u32),
        })
        .filter(|img| !img.url.is_empty())
        .collect()
}

/// Track artists: `artists.items[]` with `uri` + `profile.name`.
fn parse_artists(track_data: &Value) -> Vec<crate::spotify::models::ArtistRef> {
    track_data
        .get("artists")
        .and_then(|a| a.get("items"))
        .and_then(|a| a.as_array())
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|artist| {
            let uri = str(&artist["uri"]);
            if uri.is_empty() {
                return None;
            }
            Some(crate::spotify::models::ArtistRef {
                id: uri_part(&artist["uri"]),
                name: artist["profile"]["name"].as_str().unwrap_or_default().to_string(),
                uri,
            })
        })
        .collect()
}

/// Parse a GQL track node (the `data` inside a `{ track: { _uri, data } }` or
/// wrapper with `_uri`).
fn parse_gql_track(track_data: &Value, uri_override: Option<&str>) -> Option<crate::spotify::models::Track> {
    let uri = match uri_override {
        Some(u) => u.to_string(),
        None => {
            let u = track_data["uri"].as_str().unwrap_or_default();
            if u.is_empty() {
                str(&track_data["_uri"]).to_string()
            } else {
                u.to_string()
            }
        }
    };
    if uri.is_empty() {
        return None;
    }
    let id = uri_part(&Value::String(uri.clone())).clone();
    if id.is_empty() || id == uri {
        return None;
    }

    let duration = track_data["duration"]["totalMilliseconds"]
        .as_u64()
        .or_else(|| track_data["durationMs"].as_u64())
        .or_else(|| track_data["duration_ms"].as_u64())
        .or_else(|| track_data["trackDuration"]["totalMilliseconds"].as_u64())
        .unwrap_or(0);

    let album_data = &track_data["albumOfTrack"];
    let album = crate::spotify::models::AlbumRef {
        id: uri_part(&album_data["uri"]),
        name: str(&album_data["name"]),
        uri: str(&album_data["uri"]),
        images: parse_images(&album_data["coverArt"]),
        album_type: None,
        release_date: None,
    };

    Some(crate::spotify::models::Track {
        id,
        name: str(&track_data["name"]),
        duration_ms: duration,
        artists: parse_artists(track_data),
        album,
        uri,
        explicit: track_data["contentRating"]["label"].as_str() == Some("EXPLICIT"),
        preview_url: None,
        popularity: 0,
    })
}

/// Parse playlist image groups: `images: { totalCount, items: [ { sources: [...] } ] }`.
fn parse_playlist_images(data: &Value) -> Vec<crate::spotify::models::SpotifyImage> {
    data["images"]["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .flat_map(|group| parse_images(&group))
        .collect()
}

/// Playlist track count from a GQL playlist node. `libraryV3`/`fetchPlaylist`
/// carry it under a few field names depending on the schema version; 0 means
/// "unknown" (the UI then omits the count).
fn parse_playlist_track_total(inner: &Value) -> u32 {
    inner["trackCount"]
        .as_u64()
        .or_else(|| inner["content"]["totalCount"].as_u64())
        .or_else(|| inner["totalLength"].as_u64())
        .unwrap_or(0) as u32
}

/// User playlists via `libraryV3` (filter=Playlists, flattened).
pub async fn gql_user_playlists(limit: u32, offset: u32) -> Result<Vec<crate::spotify::models::Playlist>, AppError> {
    let vars = json!({
        "filters": ["Playlists"],
        "order": null,
        "textFilter": "",
        "features": ["LIKED_SONGS", "YOUR_EPISODES_V2", "PRERELEASES", "EVENTS"],
        "limit": limit,
        "offset": offset,
        "flatten": true,
        "expandedFolders": [],
        "folderUri": null,
        "includeFoldersWhenFlattening": false
    });

    let data = graphql_post("libraryV3", hashes::LIBRARY_V3, vars).await?;
    let library = &data["me"]["libraryV3"];

    let mut playlists = Vec::new();
    if let Some(items) = library["items"].as_array() {
        for elem in items {
            let wrapper = &elem["item"];
            let inner = &wrapper["data"];
            if inner["__typename"].as_str() != Some("Playlist") {
                continue;
            }
            let uri = str(&wrapper["_uri"]);
            if uri.is_empty() {
                continue;
            }
            let owner_v2 = &inner["ownerV2"]["data"];
            let track_total = parse_playlist_track_total(inner);
            playlists.push(crate::spotify::models::Playlist {
                id: uri_part(&wrapper["_uri"]),
                name: str(&inner["name"]),
                images: parse_playlist_images(inner),
                owner: crate::spotify::models::Owner {
                    id: uri_part(&owner_v2["uri"]),
                    display_name: Some(str(&owner_v2["name"])).filter(|s| !s.is_empty()),
                },
                tracks: crate::spotify::models::TracksMeta {
                    total: track_total,
                    items: Vec::new(),
                },
                uri,
                description: str(&inner["description"]),
            });
        }
    }
    Ok(playlists)
}

/// Liked songs via `fetchLibraryTracks`.
pub async fn gql_user_liked_tracks(limit: u32, offset: u32) -> Result<Vec<crate::spotify::models::Track>, AppError> {
    let vars = json!({ "offset": offset, "limit": limit });
    let data = graphql_post("fetchLibraryTracks", hashes::FETCH_LIBRARY_TRACKS, vars).await?;
    let tracks_data = &data["me"]["library"]["tracks"];

    let mut tracks = Vec::new();
    if let Some(items) = tracks_data["items"].as_array() {
        for elem in items {
            let wrapper = &elem["track"];
            let track_data = &wrapper["data"];
            let uri_override = wrapper["_uri"].as_str().or_else(|| wrapper["uri"].as_str());
            if let Some(t) = parse_gql_track(track_data, uri_override) {
                tracks.push(t);
            }
        }
    }
    Ok(tracks)
}

fn parse_gql_album(data: &Value, uri_override: Option<&str>) -> Option<crate::spotify::models::Album> {
    let uri = match uri_override {
        Some(u) => u.to_string(),
        None => str(&data["uri"]),
    };
    if uri.is_empty() {
        return None;
    }
    let id = uri_part(&Value::String(uri.clone()));
    if id.is_empty() || id == uri {
        return None;
    }
    Some(crate::spotify::models::Album {
        id,
        name: str(&data["name"]),
        artists: parse_artists(data),
        images: parse_images(&data["coverArt"]),
        tracks: None,
        release_date: album_release_date(data),
        total_tracks: 0,
        uri,
        album_type: data["type"].as_str().map(|s| s.to_string()),
    })
}

/// Best-effort release date string from the GQL `date` object, which varies by
/// operation: libraryV3/search use `isoString`, others expose `{year}` and/or
/// `{day,month,year}`.
fn album_release_date(data: &Value) -> String {
    let d = &data["date"];
    if let Some(iso) = d["isoString"].as_str() {
        if !iso.is_empty() {
            return iso[..4].to_string();
        }
    }
    let mut parts = Vec::new();
    if let Some(day) = d["day"].as_u64() {
        parts.push(format!("{day:02}"));
    }
    if let Some(month) = d["month"].as_u64() {
        parts.push(format!("{month:02}"));
    }
    if let Some(year) = d["year"].as_u64() {
        parts.push(format!("{year:04}"));
    }
    parts.join("-")
}

/// Artist node from `searchV2.artists`: `profile.name`, `uri`,
/// `visuals.avatarImage.sources[]`.
fn parse_gql_artist(data: &Value) -> Option<crate::spotify::models::Artist> {
    let uri = str(&data["uri"]);
    if uri.is_empty() {
        return None;
    }
    let id = uri_part(&Value::String(uri.clone()));
    if id.is_empty() || id == uri {
        return None;
    }
    Some(crate::spotify::models::Artist {
        id,
        name: data["profile"]["name"].as_str().unwrap_or_default().to_string(),
        images: parse_images(&data["visuals"]["avatarImage"]),
        ..crate::spotify::models::Artist::default()
    })
}

/// Saved albums via `libraryV3` (filter=Albums). Mirrors `/me/albums` in
/// shape without the hard `/v1` rate limit.
pub async fn gql_user_albums(limit: u32, offset: u32) -> Result<Vec<crate::spotify::models::Album>, AppError> {
    let vars = json!({
        "filters": ["Albums"],
        "order": null,
        "textFilter": "",
        "features": ["LIKED_SONGS", "YOUR_EPISODES_V2", "PRERELEASES", "EVENTS"],
        "limit": limit,
        "offset": offset,
        "flatten": true,
        "expandedFolders": [],
        "folderUri": null,
        "includeFoldersWhenFlattening": false
    });

    let data = graphql_post("libraryV3", hashes::LIBRARY_V3, vars).await?;
    let library = &data["me"]["libraryV3"];

    let mut albums = Vec::new();
    if let Some(items) = library["items"].as_array() {
        for elem in items {
            let wrapper = &elem["item"];
            let inner = &wrapper["data"];
            if inner["__typename"].as_str() != Some("Album") {
                continue;
            }
            let uri_override = wrapper["_uri"].as_str();
            if let Some(a) = parse_gql_album(inner, uri_override) {
                albums.push(a);
            }
        }
    }
    Ok(albums)
}

/// Playlist detail + its tracks via `fetchPlaylist`. Populates `tracks.items`
/// and `tracks.total`, so the detail page renders (and shows a real count).
pub async fn gql_playlist(id: &str) -> Result<crate::spotify::models::Playlist, AppError> {
    let vars = json!({
        "uri": format!("spotify:playlist:{id}"),
        "offset": 0,
        "limit": 100,
        "enableWatchFeedEntrypoint": false
    });

    let data = graphql_post("fetchPlaylist", hashes::FETCH_PLAYLIST, vars).await?;
    let playlist_data = &data["playlistV2"];
    if playlist_data.is_null() {
        return Err(AppError::Spotify(format!("gql fetchPlaylist: no playlistV2 for {id}")));
    }

    let owner_v2 = &playlist_data["ownerV2"]["data"];
    let uri = format!("spotify:playlist:{id}");

    let mut tracks = Vec::new();
    if let Some(items) = playlist_data["content"]["items"].as_array() {
        for elem in items {
            let wrapper = &elem["itemV2"];
            let track_data = &wrapper["data"];
            let uri_override = wrapper["_uri"].as_str().or_else(|| wrapper["uri"].as_str());
            if let Some(t) = parse_gql_track(track_data, uri_override) {
                tracks.push(t);
            }
        }
    }
    let total = playlist_data["content"]["totalCount"].as_u64().unwrap_or(tracks.len() as u64) as u32;

    Ok(crate::spotify::models::Playlist {
        id: id.to_owned(),
        name: str(&playlist_data["name"]),
        images: parse_playlist_images(playlist_data),
        owner: crate::spotify::models::Owner {
            id: uri_part(&owner_v2["uri"]),
            display_name: Some(str(&owner_v2["name"])).filter(|s| !s.is_empty()),
        },
        tracks: crate::spotify::models::TracksMeta { total, items: tracks },
        uri,
        description: str(&playlist_data["description"]),
    })
}

/// Single track metadata via the `searchDesktop` GQL operation.
///
/// Searches by the track's exact URI — Spotify matches full URIs — so a user
/// already has the metadata in hand, but the URI-only playback fallback needs
/// this. Returns the first matching track, or `None` if nothing matched.
pub async fn gql_track(id: &str) -> Result<Option<crate::spotify::models::Track>, AppError> {
    let vars = json!({
        "searchTerm": format!("spotify:track:{id}"),
        "offset": 0,
        "limit": 5,
        "numberOfTopResults": 5,
        "includeAudiobooks": true,
        "includeArtistHasConcertsField": false,
        "includePreReleases": false,
        "includeLocalConcertsField": false,
        "includeAuthors": false
    });

    let data = graphql_post("searchDesktop", hashes::SEARCH_DESKTOP, vars).await?;
    let tracks_v2 = &data["searchV2"]["tracksV2"];
    if let Some(items) = tracks_v2["items"].as_array() {
        for elem in items {
            let wrapper = &elem["item"];
            let track_data = &wrapper["data"];
            if track_data["__typename"].as_str() != Some("Track") {
                continue;
            }
            let uri_override = wrapper["_uri"].as_str().or_else(|| wrapper["uri"].as_str());
            if let Some(t) = parse_gql_track(track_data, uri_override) {
                if t.id == id {
                    return Ok(Some(t));
                }
            }
        }
    }
    Ok(None)
}

/// Full-text search over tracks, albums, and artists via the `searchDesktop`
/// GQL operation. Same idea as `/v1/search` but routed through pathfinder so it
/// is free of the `/v1` hard 429 rate limit.
pub async fn gql_search(
    q: &str,
    limit: u32,
) -> Result<crate::spotify::models::SearchResults, AppError> {
    let vars = json!({
        "searchTerm": q,
        "offset": 0,
        "limit": limit,
        "numberOfTopResults": limit,
        "includeAudiobooks": true,
        "includeArtistHasConcertsField": false,
        "includePreReleases": false,
        "includeLocalConcertsField": false,
        "includeAuthors": false
    });

    let data = graphql_post("searchDesktop", hashes::SEARCH_DESKTOP, vars).await?;
    let sv = &data["searchV2"];

    let mut tracks = Vec::new();
    if let Some(items) = sv["tracksV2"]["items"].as_array() {
        for elem in items {
            let wrapper = &elem["item"];
            let track_data = &wrapper["data"];
            if track_data["__typename"].as_str() != Some("Track") {
                continue;
            }
            let uri_override = wrapper["_uri"].as_str().or_else(|| wrapper["uri"].as_str());
            if let Some(t) = parse_gql_track(track_data, uri_override) {
                tracks.push(t);
            }
        }
    }

    let mut albums = Vec::new();
    if let Some(items) = sv["albumsV2"]["items"].as_array() {
        for elem in items {
            let inner = &elem["data"];
            if inner["__typename"].as_str() != Some("Album") {
                continue;
            }
            if let Some(a) = parse_gql_album(inner, None) {
                albums.push(a);
            }
        }
    }

    let mut artists = Vec::new();
    if let Some(items) = sv["artists"]["items"].as_array() {
        for elem in items {
            let inner = &elem["data"];
            if inner["__typename"].as_str() != Some("Artist") {
                continue;
            }
            if let Some(a) = parse_gql_artist(inner) {
                artists.push(a);
            }
        }
    }

    Ok(crate::spotify::models::SearchResults {
        tracks: page_of(&tracks),
        albums: page_of(&albums),
        artists: page_of(&artists),
        playlists: None,
    })
}

fn page_of<T: Clone>(v: &[T]) -> Option<crate::spotify::models::Paged<T>> {
    if v.is_empty() {
        None
    } else {
        Some(crate::spotify::models::Paged {
            items: v.to_vec(),
            total: v.len() as u32,
            limit: 0,
            offset: 0,
            next: None,
            previous: None,
        })
    }
}

/// Album detail + its tracks via the `getAlbum` GQL operation (same idea as
/// `/v1/albums/{id}`, but through pathfinder so it avoids the 429 limit).
/// Populates `tracks.items` so the album page renders a real track list and
/// count.
pub async fn gql_album(id: &str) -> Result<crate::spotify::models::Album, AppError> {
    let vars = json!({
        "uri": format!("spotify:album:{id}"),
        "offset": 0,
        "limit": 100
    });

    let data = graphql_post("getAlbum", hashes::GET_ALBUM, vars).await?;
    let album_union = &data["albumUnion"];
    if album_union["__typename"].as_str() != Some("Album") {
        return Err(AppError::Spotify(format!("album {id} not found via GQL")));
    }
    let mut album = parse_gql_album(album_union, None)
        .ok_or_else(|| AppError::Spotify(format!("album {id} could not be parsed")))?;

    let mut tracks = Vec::new();
    if let Some(items) = album_union["tracksV2"]["items"].as_array() {
        for elem in items {
            let track_data = &elem["track"];
            if track_data["__typename"].as_str().is_some_and(|t| t != "Track") {
                continue;
            }
            if let Some(t) = parse_gql_track(track_data, None) {
                tracks.push(t);
            }
        }
    }
    album.tracks = Some(page_of(&tracks).unwrap_or_default());
    album.total_tracks = tracks.len() as u32;
    Ok(album)
}

/// Artist hero node from `queryArtistOverview`: `profile.name`, `uri`,
/// `visuals.avatarImage.sources[]`, `stats.followers`.
fn parse_gql_artist_hero(data: &Value) -> Option<crate::spotify::models::Artist> {
    let uri = str(&data["uri"]);
    if uri.is_empty() {
        return None;
    }
    let id = uri_part(&Value::String(uri.clone()));
    if id.is_empty() || id == uri {
        return None;
    }
    Some(crate::spotify::models::Artist {
        id,
        name: data["profile"]["name"].as_str().unwrap_or_default().to_string(),
        images: parse_images(&data["visuals"]["avatarImage"]),
        followers: crate::spotify::models::Followers {
            total: data["stats"]["followers"].as_u64().unwrap_or(0),
        },
        uri,
        ..crate::spotify::models::Artist::default()
    })
}

/// Flatten `discography.<group>.items[].releases.items[]` (albums or singles)
/// into `Album` models. The `queryArtistOverview` discography nests releases
/// inside per-group nodes; the artist page shows them as its "Discography".
fn parse_discography(data: &Value) -> Vec<crate::spotify::models::Album> {
    let mut out = Vec::new();
    for group in ["albums", "singles"] {
        if let Some(items) = data["discography"][group]["items"].as_array() {
            for group_node in items {
                if let Some(releases) = group_node["releases"]["items"].as_array() {
                    for release in releases {
                        if release["__typename"].as_str().is_some_and(|t| t != "Album") {
                            continue;
                        }
                        if let Some(a) = parse_gql_album(release, None) {
                            out.push(a);
                        }
                    }
                }
            }
        }
    }
    out
}

/// Full artist page via `queryArtistOverview` + `queryArtistRelated`, replacing
/// four separate `/v1/artists/{id}` calls with (two) pathfinder queries that
/// are free of the 429 hard limit. Returns (hero, discography, popular tracks,
/// related artists).
pub async fn gql_artist_page(
    id: &str,
) -> Result<
    (
        crate::spotify::models::Artist,
        Vec<crate::spotify::models::Album>,
        Vec<crate::spotify::models::Track>,
        Vec<crate::spotify::models::Artist>,
    ),
    AppError,
> {
    let vars = json!({ "uri": format!("spotify:artist:{id}") });

    let data = graphql_post("queryArtistOverview", hashes::QUERY_ARTIST_OVERVIEW, vars.clone()).await?;
    let artist = &data["artistUnion"];
    if artist["__typename"].as_str() != Some("Artist") {
        return Err(AppError::Spotify(format!("artist {id} not found via GQL")));
    }
    let hero = parse_gql_artist_hero(artist)
        .ok_or_else(|| AppError::Spotify(format!("artist {id} could not be parsed")))?;
    let albums = parse_discography(artist);
    let mut top_tracks = Vec::new();
    if let Some(items) = artist["discography"]["topTracks"]["items"].as_array() {
        for elem in items {
            let track_data = &elem["track"];
            if let Some(t) = parse_gql_track(track_data, None) {
                top_tracks.push(t);
            }
        }
    }

    let related = gql_artist_related(id).await.unwrap_or_default();

    Ok((hero, albums, top_tracks, related))
}

/// "Fans also like" artists via `queryArtistRelated`.
pub async fn gql_artist_related(id: &str) -> Result<Vec<crate::spotify::models::Artist>, AppError> {
    let vars = json!({
        "uri": format!("spotify:artist:{id}"),
        "offset": 0,
        "limit": 12
    });
    let data = graphql_post("queryArtistRelated", hashes::QUERY_ARTIST_RELATED, vars).await?;
    let mut artists = Vec::new();
    if let Some(items) = data["artistUnion"]["relatedContent"]["relatedArtists"]["items"].as_array() {
        for node in items {
            if let Some(a) = parse_gql_artist(node) {
                artists.push(a);
            }
        }
    }
    Ok(artists)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_gql_track_flat() {
        let v = json!({
            "uri": "spotify:track:abc",
            "name": "Mercy",
            "duration": { "totalMilliseconds": 200000 },
            "artists": { "items": [
                { "uri": "spotify:artist:a1", "profile": { "name": "Kanye" } }
            ]},
            "albumOfTrack": {
                "uri": "spotify:album:al1",
                "name": "Yeezus",
                "coverArt": { "sources": [
                    { "url": "https://i.scdn.co/img", "width": 640, "height": 640 }
                ]}
            },
            "contentRating": { "label": "EXPLICIT" }
        });
        let t = parse_gql_track(&v, None).unwrap();
        assert_eq!(t.id, "abc");
        assert_eq!(t.name, "Mercy");
        assert_eq!(t.duration_ms, 200000);
        assert_eq!(t.artists[0].id, "a1");
        assert_eq!(t.artists[0].name, "Kanye");
        assert_eq!(t.album.id, "al1");
        assert_eq!(t.album.images[0].url, "https://i.scdn.co/img");
        assert!(t.explicit);
    }

    #[test]
    fn parse_gql_track_nested_wrapper_uses_override_uri() {
        let wrapper = json!({ "_uri": "spotify:track:xyz", "data": { "name": "X" } });
        let t = parse_gql_track(&wrapper["data"], wrapper["_uri"].as_str()).unwrap();
        assert_eq!(t.id, "xyz");
    }

    #[test]
    fn parse_playlist_images_flattens_groups() {
        let data = json!({
            "images": { "totalCount": 1, "items": [
                { "sources": [
                    { "url": "https://i.scdn.co/a", "width": 300, "height": 300 },
                    { "url": "https://i.scdn.co/b", "width": 640, "height": 640 }
                ]}
            ]}
        });
        let imgs = parse_playlist_images(&data);
        assert_eq!(imgs.len(), 2);
        assert_eq!(imgs[1].width, Some(640));
    }

    #[test]
    fn parse_library_playlist_reads_track_count() {
        let inner = json!({
            "__typename": "Playlist",
            "name": "Gooood",
            "trackCount": 33,
            "ownerV2": { "data": { "uri": "spotify:user:someone", "name": "Me" } }
        });
        assert_eq!(parse_playlist_track_total(&inner), 33);
        let inner2 = json!({ "content": { "totalCount": 7 } });
        assert_eq!(parse_playlist_track_total(&inner2), 7);
        let inner3 = json!({});
        assert_eq!(parse_playlist_track_total(&inner3), 0);
    }

    #[test]
    fn parse_gql_album_reads_search_shape() {
        let v = json!({
            "__typename": "Album",
            "uri": "spotify:album:al1",
            "name": "Starboy",
            "type": "ALBUM",
            "coverArt": { "sources": [ { "url": "https://i.scdn.co/c", "width": 640, "height": 640 } ] },
            "date": { "year": 2016 },
            "artists": { "items": [ { "uri": "spotify:artist:a1", "profile": { "name": "Weeknd" } } ] }
        });
        let a = parse_gql_album(&v, None).unwrap();
        assert_eq!(a.id, "al1");
        assert_eq!(a.name, "Starboy");
        assert_eq!(a.release_date, "2016");
        assert_eq!(a.images[0].url, "https://i.scdn.co/c");
        assert_eq!(a.artists[0].name, "Weeknd");
    }

    #[test]
    fn parse_discography_flattens_album_and_single_groups() {
        let v = json!({
            "discography": {
                "albums": { "items": [ { "releases": { "items": [
                    { "__typename": "Album", "uri": "spotify:album:al1", "name": "A" }
                ] } } ] },
                "singles": { "items": [ { "releases": { "items": [
                    { "__typename": "Album", "uri": "spotify:album:al2", "name": "S" }
                ] } } ] },
                "compilations": { "items": [ { "releases": { "items": [
                    { "__typename": "Album", "uri": "spotify:album:co1", "name": "C" }
                ] } } ] }
            }
        });
        let albums = parse_discography(&v);
        let ids: Vec<&str> = albums.iter().map(|a| a.id.as_str()).collect();
        // Compilations are intentionally excluded from the artist shelf.
        assert_eq!(ids, vec!["al1", "al2"]);
    }

    #[test]
    fn parse_gql_artist_hero_reads_stats_and_avatar() {
        let v = json!({
            "__typename": "Artist",
            "uri": "spotify:artist:ar1",
            "profile": { "name": "Future" },
            "stats": { "followers": 25136525 },
            "visuals": { "avatarImage": { "sources": [
                { "url": "https://i.scdn.co/av", "width": 640, "height": 640 }
            ] } }
        });
        let a = parse_gql_artist_hero(&v).unwrap();
        assert_eq!(a.id, "ar1");
        assert_eq!(a.name, "Future");
        assert_eq!(a.followers.total, 25136525);
        assert_eq!(a.images[0].url, "https://i.scdn.co/av");
    }

    #[test]
    fn parse_gql_artist_reads_related_node() {
        let v = json!({
            "__typename": "Artist",
            "uri": "spotify:artist:ar2",
            "profile": { "name": "Young Thug" },
            "visuals": { "avatarImage": { "sources": [
                { "url": "https://i.scdn.co/yt", "width": 320, "height": 320 }
            ] } }
        });
        let a = parse_gql_artist(&v).unwrap();
        assert_eq!(a.id, "ar2");
        assert_eq!(a.name, "Young Thug");
        assert_eq!(a.images[0].url, "https://i.scdn.co/yt");
    }
}
