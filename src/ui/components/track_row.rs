use dioxus::prelude::*;

use crate::spotify::models::Track;
use crate::ui::components::AlbumArt;

/// One row in a track / saved-songs list.
#[component]
pub fn TrackRow(track: Track, index: Option<u32>, onplay: EventHandler<Track>) -> Element {
    let name = track.name.clone();
    let artist_line = track
        .artists
        .iter()
        .map(|artist| artist.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let duration_ms = track.duration_ms;
    let duration = crate::state::format_duration(duration_ms);

    let artwork_url = crate::ui::components::pick_artwork(&track.album.images, 64);

    let album_art_seed = track.id.clone();
    let played = track.clone();
    let row_class = if index.is_some() {
        "track-row"
    } else {
        "track-row track-row--noindex"
    };
    rsx! {
        // A real button (not a clickable div): keyboard-focusable and
        // operable, matching the :focus-within rules the stylesheet
        // already defines for rows that could never take focus.
        button {
            class: "{row_class}",
            onclick: move |_| onplay.call(played.clone()),
            if let Some(index) = index {
                span { class: "track-index", "{index}" }
            }
            AlbumArt { url: artwork_url, seed: album_art_seed, class: Some("track-row-art".to_string()) }
            div { class: "track-row-title",
                div { class: "track-name", title: "{name}", "{name}" }
                div { class: "track-artists", title: "{artist_line}", "{artist_line}" }
            }
            div { class: "track-duration", "{duration}" }
        }
    }
}
