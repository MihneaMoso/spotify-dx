use dioxus::prelude::*;

use crate::spotify::api;
use crate::ui::components::{HeroHeader, TrackTable};

/// Playlist detail: hero + full track table.
#[component]
pub fn Playlist(id: String) -> Element {
    let resource = use_resource(move || {
        let id = id.clone();
        async move { api::get_playlist(&id).await }
    });

    // Clone the Ok payload out so no read guard crosses the rsx return.
    let loaded = resource
        .read()
        .as_ref()
        .and_then(|r| r.as_ref().ok())
        .cloned();
    let is_err = matches!(resource.read().as_ref(), Some(Err(_)));

    if let Some(playlist) = loaded {
        let name = playlist.name.clone();
        let owner = playlist.owner.display_name.clone().unwrap_or_default();
        let meta = format!("{} songs · by {}", playlist.tracks.total, owner);
        let art_url = playlist.images.first().map(|i| i.url.clone()).unwrap_or_default();
        let first_uri = playlist
            .tracks
            .items
            .first()
            .map(|t| t.uri.clone())
            .unwrap_or_default();
        let tracks = playlist.tracks.items.clone();
        let shuffle_tracks = tracks.clone();

        rsx! {
            div { class: "page detail",
                HeroHeader {
                    kind: "Playlist".to_string(),
                    title: name,
                    meta: meta,
                    image_url: art_url,
                    seed: playlist.id.clone(),
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
    } else if is_err {
        rsx! { div { class: "page detail", div { class: "error-banner", "Playlist unavailable." } } }
    } else {
        rsx! { div { class: "page detail", div { class: "page-spinner", div { class: "spinner" } } } }
    }
}