use dioxus::prelude::*;

use crate::spotify::api;
use crate::ui::components::{AlbumArt, TrackRow};
use crate::ui::router::Route;

/// Album detail page: artwork hero, metadata and the track list.
#[component]
pub fn Album(id: String) -> Element {
    let navigator = use_navigator();
    let resource = use_resource(move || {
        let id = id.clone();
        async move { api::get_album(&id).await }
    });

    let snapshot = match resource.read().as_ref() {
        Some(Ok(album)) => album.clone(),
        _ => crate::spotify::models::Album::default(),
    };

    let name = snapshot.name.clone();
    let artists = snapshot
        .artists
        .iter()
        .map(|a| a.name.clone())
        .collect::<Vec<_>>()
        .join(", ");
    let release_year = snapshot.release_date.split('-').next().unwrap_or("").to_string();
    let tracks = snapshot.tracks.as_ref().map(|page| page.items.clone()).unwrap_or_default();
    let total = tracks.len();
    let art_url = snapshot
        .images
        .first()
        .map(|img| img.url.clone())
        .unwrap_or_default();
    let art_seed = snapshot.id.clone();
    let playable = !snapshot.id.is_empty();
    let play_uri = format!("spotify:album:{}", snapshot.id);

    let rows = tracks
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
                div { class: "error-banner", "Album unavailable." }
            } else {
                div { class: "detail-hero",
                    AlbumArt { url: art_url, seed: art_seed, class: Some("detail-art".to_string()) }
                    div { class: "detail-text",
                        h1 { "{name}" }
                        div { class: "detail-subtitle",
                            "{artists} · {release_year} · {total} tracks"
                        }
                        div { class: "detail-actions",
                            button {
                                class: "primary",
                                onclick: move |_| {
                                    // Seed the "playing" state so the bar isn't
                                    // empty while the connect session spins up.
                                    crate::player::launch(play_uri.clone());
                                },
                                "Play album"
                            }
                            button {
                                class: "ghost",
                                onclick: move |_| { navigator.push(Route::Home); },
                                "Back home"
                            }
                        }
                    }
                }
                div { class: "track-list", for row in rows { {row} } }
            }
        }
    }
}