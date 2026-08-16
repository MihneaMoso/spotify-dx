use dioxus::prelude::*;

use crate::spotify::api;
use crate::ui::components::{AlbumArt, TrackRow};
use crate::ui::router::Route;

/// Artist page: header artwork, biography summary and top tracks.
#[component]
pub fn Artist(id: String) -> Element {
    let navigator = use_navigator();
    let resource_id = id.clone();
    let resource = use_resource(move || {
        let id = resource_id.clone();
        async move { api::get_artist(&id).await }
    });

    let artist = match resource.read().as_ref() {
        Some(Ok(artist)) => artist.clone(),
        _ => crate::spotify::models::Artist::default(),
    };

    let top_tracks_id = id.clone();
    let top_tracks = use_resource(move || {
        let id = top_tracks_id.clone();
        async move { api::get_artist_top_tracks(&id).await.unwrap_or_default() }
    });

    let name = artist.name.clone();
    let genres = artist.genres.join(" · ");
    let art_url = artist
        .images
        .first()
        .map(|img| img.url.clone())
        .unwrap_or_default();
    let art_seed = artist.id.clone();
    let playable = !artist.id.is_empty();

    let rows = top_tracks
        .read()
        .as_ref()
        .map(|tracks| {
            tracks
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
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    rsx! {
        div { class: "page detail",
            if resource.read().is_none() {
                div { class: "page-spinner", div { class: "spinner" } }
            } else if !playable {
                div { class: "error-banner", "Artist unavailable." }
            } else {
                div { class: "detail-hero",
                    AlbumArt { url: art_url, seed: art_seed, class: Some("detail-art".to_string()) }
                    div { class: "detail-text",
                        h1 { "{name}" }
                        div { class: "detail-subtitle", "{genres}" }
                        div { class: "detail-actions",
                            button {
                                class: "primary",
                                onclick: move |_| { navigator.push(Route::ArtistTopTracks { id: id.clone() }); },
                                "Top tracks"
                            }
                            button {
                                class: "ghost",
                                onclick: move |_| { navigator.push(Route::Home); },
                                "Back home"
                            }
                        }
                    }
                }
                div { class: "detail-section", h2 { "Popular" } }
                div { class: "track-list", for row in rows { {row} } }
            }
        }
    }
}

/// Lightweight "top tracks" route for an artist (reuses the same layout).
#[component]
pub fn ArtistTopTracks(id: String) -> Element {
    let navigator = use_navigator();
    let id_for_resource = id.clone();
    let resource = use_resource(move || {
        let id = id_for_resource.clone();
        async move { api::get_artist_top_tracks(&id).await.unwrap_or_default() }
    });

    let rows = resource
        .read()
        .as_ref()
        .map(|tracks| {
            tracks
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
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    rsx! {
        div { class: "page detail",
            div { class: "detail-header-bar",
                button {
                    class: "ghost",
                    onclick: move |_| { navigator.push(Route::Artist { id: id.clone() }); },
                    "⇦ Back"
                }
            }
            div { class: "track-list", for row in rows { {row} } }
        }
    }
}