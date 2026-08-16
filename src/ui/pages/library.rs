use dioxus::prelude::*;

use crate::app_error::AppError;
use crate::spotify::api;
use crate::ui::components::{MediaCard, TrackRow};
use crate::ui::router::Route;

/// "Your Library": saved albums, followed playlists and liked songs.
#[component]
pub fn Library() -> Element {
    let navigator = use_navigator();
    let resource = use_resource(|| async move {
        let mut first_error: Option<AppError> = None;
        let playlists = match api::get_user_playlists().await {
            Ok(list) => list,
            Err(err) => {
                if first_error.is_none() {
                    first_error = Some(err);
                }
                Vec::new()
            }
        };
        let albums = match api::get_user_albums(50, 0).await {
            Ok(list) => list,
            Err(err) => {
                if first_error.is_none() {
                    first_error = Some(err);
                }
                Vec::new()
            }
        };
        let tracks = match api::get_user_saved_tracks(50, 0).await {
            Ok(page) => page.items,
            Err(err) => {
                if first_error.is_none() {
                    first_error = Some(err);
                }
                Vec::new()
            }
        };
        (playlists, albums, tracks, first_error)
    });

    let album_cards: Vec<Element> = match resource.read().as_ref() {
        Some((_, albums, _, _)) => albums
            .iter()
            .map(|a| {
                let name = a.name.clone();
                let subtitle = a.artists.first().map(|x| x.name.clone()).unwrap_or_default();
                let image = a.images.first().map(|img| img.url.clone()).unwrap_or_default();
                let seed = a.id.clone();
                let id = a.id.clone();
                rsx! {
                    MediaCard {
                        title: name,
                        subtitle: subtitle,
                        image_url: image,
                        seed: seed,
                        onselect: move |_| { navigator.push(Route::Album { id: id.clone() }); },
                    }
                }
            })
            .collect(),
        _ => Vec::new(),
    };

    let playlist_cards: Vec<Element> = match resource.read().as_ref() {
        Some((playlists, _, _, _)) => playlists
            .iter()
            .map(|p| {
                let name = p.name.clone();
                let subtitle = format!("by {}", p.owner.display_name.clone().unwrap_or_default());
                let image = p.images.first().map(|img| img.url.clone()).unwrap_or_default();
                let seed = p.id.clone();
                let id = p.id.clone();
                rsx! {
                    MediaCard {
                        title: name,
                        subtitle: subtitle,
                        image_url: image,
                        seed: seed,
                        onselect: move |_| { navigator.push(Route::Playlist { id: id.clone() }); },
                    }
                }
            })
            .collect(),
        _ => Vec::new(),
    };

    let liked_rows: Vec<Element> = match resource.read().as_ref() {
        Some((_, _, tracks, _)) => tracks
            .iter()
            .map(|t| {
                let track = t.clone();
                let uri = track.uri.clone();
                rsx! {
                    TrackRow {
                        track: track,
                        index: None,
                        onplay: move |_| crate::player::launch(uri.clone()),
                    }
                }
            })
            .collect(),
        _ => Vec::new(),
    };

    rsx! {
        div { class: "page library",
            header { class: "page-header",
                h1 { "Your Library" }
            }

            if resource.read().is_none() {
                div { class: "page-spinner", div { class: "spinner" } }
            } else if let Some((_, _, _, Some(err))) = resource.read().as_ref() {
                div { class: "error-banner",
                    {err.to_string()}
                }
            } else {
                section { class: "shelf",
                    h2 { "Saved albums" }
                    div { class: "shelf-row",
                        for card in album_cards { {card} }
                    }
                }
                section { class: "shelf",
                    h2 { "Playlists" }
                    div { class: "shelf-row",
                        for card in playlist_cards { {card} }
                    }
                }
                section { class: "shelf",
                    h2 { "Liked songs" }
                    div { class: "track-list",
                        for row in liked_rows { {row} }
                    }
                }
            }
        }
    }
}