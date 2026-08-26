use serde::{Deserialize, Serialize};

/// A single playable track.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Track {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub artists: Vec<ArtistRef>,
    #[serde(default)]
    pub album: AlbumRef,
    #[serde(default)]
    pub uri: String,
    #[serde(default)]
    pub explicit: bool,
    #[serde(default)]
    pub preview_url: Option<String>,
    #[serde(default)]
    pub popularity: u32,
}

/// An album (browse view or full album with tracks).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Album {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub artists: Vec<ArtistRef>,
    #[serde(default)]
    pub images: Vec<SpotifyImage>,
    #[serde(default)]
    pub tracks: Option<Paged<Track>>,
    #[serde(default)]
    pub release_date: String,
    #[serde(default)]
    pub total_tracks: u32,
    #[serde(default)]
    pub uri: String,
    #[serde(default)]
    pub album_type: Option<String>,
}

/// An artist profile page.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Artist {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub images: Vec<SpotifyImage>,
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default)]
    pub followers: Followers,
    #[serde(default)]
    pub uri: String,
    #[serde(default)]
    pub popularity: u32,
}

/// A playlist (browse view or full playlist).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Playlist {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub images: Vec<SpotifyImage>,
    #[serde(default)]
    pub owner: Owner,
    #[serde(default)]
    pub tracks: TracksMeta,
    #[serde(default)]
    pub uri: String,
    #[serde(default)]
    pub description: String,
}

/// The minimal artist reference embedded in tracks & albums.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ArtistRef {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub uri: String,
}

/// The minimal album reference embedded in tracks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct AlbumRef {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub uri: String,
    #[serde(default)]
    pub images: Vec<SpotifyImage>,
    #[serde(default)]
    pub album_type: Option<String>,
    #[serde(default)]
    pub release_date: Option<String>,
}

/// Artwork / avatar asset.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SpotifyImage {
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Followers {
    #[serde(default)]
    pub total: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Owner {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub display_name: Option<String>,
}

/// The `tracks` object in a playlist only carries `total` (and optionally items).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct TracksMeta {
    #[serde(default)]
    pub total: u32,
    #[serde(default)]
    pub items: Vec<Track>,
}

/// Standard paginated envelope.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Paged<T> {
    #[serde(default)]
    pub items: Vec<T>,
    #[serde(default)]
    pub total: u32,
    #[serde(default)]
    pub limit: u32,
    #[serde(default)]
    pub offset: u32,
    #[serde(default)]
    pub next: Option<String>,
    #[serde(default)]
    pub previous: Option<String>,
}

/// Aggregated search results.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SearchResults {
    #[serde(default, rename = "tracks")]
    pub tracks: Option<Paged<Track>>,
    #[serde(default, rename = "albums")]
    pub albums: Option<Paged<Album>>,
    #[serde(default, rename = "artists")]
    pub artists: Option<Paged<Artist>>,
    #[serde(default, rename = "playlists")]
    pub playlists: Option<Paged<Playlist>>,
}

/// Bundle of `home` feed sections.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct HomeData {
    pub playlists: Vec<Playlist>,
    pub liked_tracks: Vec<Track>,
}

/// The authenticated user's public profile.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct UserProfile {
    pub id: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub images: Vec<SpotifyImage>,
    #[serde(default)]
    pub product: Option<String>,
}

/// One entry of `/me/tracks`: Spotify wraps the track with the date it was
/// liked. The inner `track` can be null for region-blocked entries.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SavedTrack {
    #[serde(default)]
    pub added_at: String,
    #[serde(default)]
    pub track: Option<Track>,
}

impl SavedTrack {
    /// The playable track, if this entry has one.
    pub fn playable(&self) -> Option<&Track> {
        self.track.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_track_parses_spotify_envelope() {
        let json = serde_json::json!({
            "added_at": "2026-01-02T03:04:05Z",
            "track": {
                "id": "t1", "name": "Mercy", "duration_ms": 200_000,
                "artists": [{"id": "a", "name": "Kanye"}]
            }
        });
        let saved: SavedTrack = serde_json::from_value(json).unwrap();
        assert_eq!(saved.added_at, "2026-01-02T03:04:05Z");
        assert_eq!(saved.playable().unwrap().name, "Mercy");
    }

    #[test]
    fn saved_track_tolerates_null_track() {
        let json = serde_json::json!({"added_at": "2026-01-02T03:04:05Z", "track": null});
        let saved: SavedTrack = serde_json::from_value(json).unwrap();
        assert!(saved.playable().is_none());
    }

    #[test]
    fn paged_saved_tracks_parses_mixed_entries() {
        let json = serde_json::json!({
            "items": [
                {"added_at": "a", "track": {"id": "1", "name": "One"}},
                {"added_at": "b", "track": null}
            ],
            "total": 2, "limit": 50, "offset": 0
        });
        let page: Paged<SavedTrack> = serde_json::from_value(json).unwrap();
        assert_eq!(page.items.len(), 2);
        assert_eq!(page.total, 2);
        assert!(page.items[0].playable().is_some());
        assert!(page.items[1].playable().is_none());
    }
}