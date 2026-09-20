use dioxus::prelude::*;

use crate::spotify::api;
use crate::ui::components::{HeroHeader, TrackTable};

/// Album detail: hero (year · artist) + track table.
#[component]
pub fn Album(id: String) -> Element {
    let resource = use_resource(move || {
        let id = id.clone();
        async move { api::get_album(&id).await }
    });

    // Clone the Ok payload out so no read guard crosses the nested use_resource.
    let album_loaded = resource
        .read()
        .as_ref()
        .and_then(|r| r.as_ref().ok())
        .cloned();
    let is_err = matches!(resource.read().as_ref(), Some(Err(_)));

    // Hoisted above the match: hooks must run identically on every render —
    // a resource inside the Some arm changes the hook count on the
    // None→Some transition. None id yields an empty list (no fetch).
    let tracks_album_id = album_loaded.as_ref().map(|a| a.id.clone());
    let tracks_resource = use_resource(move || {
        let id = tracks_album_id.clone();
        async move {
            match id {
                Some(id) => api::get_album_tracks(&id).await.unwrap_or_default(),
                None => Vec::new(),
            }
        }
    });

    match album_loaded {
        None => {
            if is_err {
                rsx! { div { class: "page detail", div { class: "error-banner", "Album unavailable." } } }
            } else {
                rsx! { div { class: "page detail", div { class: "page-spinner", div { class: "spinner" } } } }
            }
        }
        Some(album) => {
            let year = album
                .release_date
                .split('-')
                .next()
                .unwrap_or(&album.release_date)
                .to_string();
            let artist_name = album
                .artists
                .first()
                .map(|a| a.name.clone())
                .unwrap_or_default();
            let meta = format!("{} · {}", year, artist_name);
            let art_url = album
                .images
                .first()
                .map(|i| i.url.clone())
                .unwrap_or_default();

            let tracks = tracks_resource.read().as_ref().cloned().unwrap_or_default();
            let total_label = album
                .tracks
                .as_ref()
                .map(|p| p.total)
                .unwrap_or(tracks.len() as u32);

            let first_track = tracks.first().cloned();
            let shuffle_tracks = tracks.clone();
            let tracks_empty = tracks.is_empty();

            rsx! {
                div { class: "page detail",
                    HeroHeader {
                        kind: "Album".to_string(),
                        title: album.name.clone(),
                        meta: format!("{meta} · {total_label} songs"),
                        image_url: art_url,
                        seed: album.id.clone(),
                        onplay: move |_| {
                            if let Some(t) = first_track.clone() {
                                crate::player::launch_track(t);
                            }
                        },
                        onshuffle: move |_| {
                            crate::player::launch_random(shuffle_tracks.clone());
                        },
                    }
                    TrackTable { tracks: tracks, numbered: true }
                    if tracks_empty {
                        div { class: "empty-state", "This album has no tracks." }
                    }
                }
            }
        }
    }
}
