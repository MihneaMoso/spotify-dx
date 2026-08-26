pub mod api;
pub mod client;
pub mod models;
pub mod player_api;
pub mod session;
pub mod store;

/// Straightforward `Result` alias used across the crate.
pub type Result<T> = std::result::Result<T, crate::app_error::AppError>;

#[cfg(test)]
mod tests {
    use crate::spotify::models::*;

    const SAMPLE_TRACK: &str = r#"{
        "id": "4NHQUGzhtTLFpeF5Z4S3zy",
        "name": "Mercy",
        "duration_ms": 267111,
        "artists": [
            { "id": "1uNFoZAHBGtllmzznpCI3s", "name": "Kanye West" }
        ],
        "album": {
            "id": "5g2x9x0h4qES2B9qY6Y0p5",
            "name": "The Life of Pablo",
            "images": [ { "url": "https://i.scdn.co/image/ab67616d00001e02f3f0", "width": 300, "height": 300 } ],
            "release_date": "2016-02-14"
        },
        "uri": "spotify:track:4NHQUGzhtTLFpeF5Z4S3zy",
        "explicit": true,
        "preview_url": "https://p.scdn.co/mp3-preview/x",
        "popularity": 88
    }"#;

    const SAMPLE_ALBUM: &str = r#"{
        "id": "5g2x9x0h4qES2B9qY6Y0p5",
        "name": "The Life of Pablo",
        "artists": [ { "id": "1uNFoZAHBGtllmzznpCI3s", "name": "Kanye West" } ],
        "images": [ { "url": "https://i.scdn.co/image/x", "width": 640, "height": 640 } ],
        "release_date": "2016-02-14",
        "total_tracks": 20,
        "uri": "spotify:album:5g2x9x0h4qES2B9qY6Y0p5",
        "tracks": {
            "items": [],
            "total": 20,
            "limit": 50,
            "offset": 0,
            "next": null,
            "previous": null
        }
    }"#;

    const SAMPLE_ARTIST: &str = r#"{
        "id": "1uNFoZAHBGtllmzznpCI3s",
        "name": "Kanye West",
        "images": [ { "url": "https://i.scdn.co/image/ar" } ],
        "genres": ["chicago rap", "rap"],
        "followers": { "total": 40000000 },
        "uri": "spotify:artist:1uNFoZAHBGtllmzznpCI3s",
        "popularity": 95
    }"#;

    #[test]
    fn test_models_deserialize_track() {
        let track: Track = serde_json::from_str(SAMPLE_TRACK).expect("track parses");
        assert_eq!(track.id, "4NHQUGzhtTLFpeF5Z4S3zy");
        assert_eq!(track.name, "Mercy");
        assert_eq!(track.duration_ms, 267111);
        assert_eq!(track.artists[0].name, "Kanye West");
        assert!(track.explicit);
        assert_eq!(track.preview_url.as_deref(), Some("https://p.scdn.co/mp3-preview/x"));
    }

    #[test]
    fn test_models_deserialize_album() {
        let album: Album = serde_json::from_str(SAMPLE_ALBUM).expect("album parses");
        assert_eq!(album.name, "The Life of Pablo");
        assert_eq!(album.total_tracks, 20);
        assert_eq!(album.tracks.as_ref().map(|p| p.total), Some(20));
    }

    #[test]
    fn test_models_deserialize_artist() {
        let artist: Artist = serde_json::from_str(SAMPLE_ARTIST).expect("artist parses");
        assert_eq!(artist.name, "Kanye West");
        assert_eq!(artist.genres, vec!["chicago rap".to_string(), "rap".to_string()]);
        assert_eq!(artist.followers.total, 40_000_000);
    }

    #[test]
    fn test_models_deserialize_partial() {
        // Missing optional fields (e.g. `preview_url`/`images`) must not break.
        let track: Track = serde_json::from_str(r#"{
            "id": "x", "name": "n", "uri": "spotify:track:x"
        }"#)
        .expect("partial track parses");
        assert_eq!(track.name, "n");
        assert_eq!(track.duration_ms, 0);
    }

    #[test]
    fn test_models_search_results_shape() {
        let value: serde_json::Value = serde_json::json!({
            "tracks": { "items": [], "total": 0, "limit": 20, "offset": 0 },
        });
        let results: SearchResults =
            serde_json::from_value(value).expect("search results parse");
        assert!(results.albums.is_none());
        assert!(results.tracks.is_some());
    }
}