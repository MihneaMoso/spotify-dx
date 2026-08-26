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
            let artist_name = album.artists.first().map(|a| a.name.clone()).unwrap_or_default();
            let meta = format!("{} · {}", year, artist_name);
            let art_url = album.images.first().map(|i| i.url.clone()).unwrap_or_default();

            let album_for_tracks = album.clone();
            let tracks_resource = use_resource(move || {
                let id = album_for_tracks.id.clone();
                async move { api::get_album_tracks(&id).await.unwrap_or_default() }
            });
            let tracks = tracks_resource.read().as_ref().cloned().unwrap_or_default();
            let total_label = album
                .tracks
                .as_ref()
                .map(|p| p.total)
                .unwrap_or(tracks.len() as u32);

            let first_uri = tracks.first().map(|t| t.uri.clone()).unwrap_or_default();
            let shuffle_tracks = tracks.clone();

            rsx! {
                div { class: "page detail",
                    HeroHeader {
                        kind: "Album".to_string(),
                        title: album.name.clone(),
                        meta: format!("{meta} · {total_label} songs"),
                        image_url: art_url,
                        seed: album.id.clone(),
                        onplay: move |_| {
                            if !first_uri.is_empty() {
                                crate::player::launch(first_uri.clone());
                            }
                        },
                        onshuffle: move |_| {
                            if !shuffle_tracks.is_empty() {
                                let i = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .map(|d| d.subsec_nanos() as usize)
                                    .unwrap_or(0);
                                crate::player::launch(shuffle_tracks[i % shuffle_tracks.len()].uri.clone());
                            }
                        },
                    }
                    TrackTable { tracks: tracks, numbered: true }
                }
            }
        }
    }
}