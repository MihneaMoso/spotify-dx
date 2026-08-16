use dioxus::prelude::*;

use crate::spotify::api;
use crate::ui::components::{AlbumArt, TrackRow};
use crate::ui::router::Route;

/// Playlist detail: hero artwork, track list, and a "play playlist" button.
#[component]
pub fn Playlist(id: String) -> Element {
    let navigator = use_navigator();
    let resource = use_resource(move || {
        let id = id.clone();
        async move { api::get_playlist(&id).await }
    });

    let snapshot = match resource.read().as_ref() {
        Some(Ok(restored)) => restored.clone(),
        _ => crate::spotify::models::Playlist::default(),
    };

    let name = snapshot.name.clone();
    let owner = snapshot.owner.display_name.clone().unwrap_or_default();
    let total = snapshot.tracks.total;
    let art_url = snapshot
        .images
        .first()
        .map(|img| img.url.clone())
        .unwrap_or_default();
    let art_seed = snapshot.id.clone();
    let playable = !snapshot.id.is_empty();
    let play_uri = format!("spotify:playlist:{}", snapshot.id);

    let rows = snapshot
        .tracks
        .items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let track = item.clone();
            let uri = track.uri.clone();
            rsx! {
                TrackRow {
                    track: track,
                    index: Some((index + 1) as u32),
                    onplay: move |_| crate::player::launch(uri.clone()),
                }
            }
        })
        .collect::<Vec<_>>();

    rsx! {
        div { class: "page detail",
            if resource.read().is_none() {
                div { class: "page-spinner", div { class: "spinner" } }
            } else if !playable {
                div { class: "error-banner", "Playlist unavailable." }
            } else {
                div { class: "detail-hero",
                    AlbumArt { url: art_url, seed: art_seed, class: Some("detail-art".to_string()) }
                    div { class: "detail-text",
                        h1 { "{name}" }
                        div { class: "detail-subtitle", "by {owner} · {total} tracks" }
                        div { class: "detail-actions",
                            button {
                                class: "primary",
                                onclick: move |_| crate::player::launch(play_uri.clone()),
                                "Play playlist"
                            }
                            button {
                                class: "ghost",
                                onclick: move |_| { navigator.push(Route::Library); },
                                "Back to library"
                            }
                        }
                    }
                }
                div { class: "track-list", for row in rows { {row} } }
            }
        }
    }
}