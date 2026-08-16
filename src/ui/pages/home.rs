use dioxus::prelude::*;

use crate::spotify::api;
use crate::ui::components::{MediaCard, TrackRow};
use crate::ui::router::Route;

/// Landing page: featured playlists, new releases and a recommended track list.
#[component]
pub fn Home() -> Element {
    let navigator = use_navigator();
    let resource = use_resource(|| async move { api::get_home().await });

    let featured_cards: Vec<Element> = match resource.read().as_ref() {
        Some(Ok(home)) => home
            .featured
            .iter()
            .map(|p| {
                let name = p.name.clone();
                let subtitle = format!("{} tracks · by {}", p.tracks.total, p.owner.display_name.clone().unwrap_or_default());
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

    let release_cards: Vec<Element> = match resource.read().as_ref() {
        Some(Ok(home)) => home
            .new_releases
            .iter()
            .map(|a| {
                let name = a.name.clone();
                let year = a.release_date.split('-').next().unwrap_or(&a.release_date).to_string();
                let artist = a.artists.first().map(|x| x.name.clone()).unwrap_or_default();
                let subtitle = format!("{year} · {artist}");
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

    let recommended_rows: Vec<Element> = match resource.read().as_ref() {
        Some(Ok(home)) => home
            .recommended
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
        div { class: "page home",
            header { class: "page-header",
                h1 { "Home" }
                span { class: "subhead", "Fresh picks, tailored to you" }
            }

            if resource.read().is_none() {
                div { class: "page-spinner", div { class: "spinner" } }
            } else if let Some(Err(err)) = resource.read().as_ref() {
                div { class: "error-banner",
                    {err.to_string()}
                    div { class: "error-detail",
                        "Couldn't load your feed. Make sure you're signed in and online."
                    }
                }
            } else if featured_cards.is_empty() {
                div { class: "error-banner",
                    "Couldn't load your feed. Make sure you're signed in and online."
                }
            } else {
                section { class: "shelf",
                    h2 { "Featured playlists" }
                    div { class: "shelf-row",
                        for card in featured_cards { {card} }
                    }
                }
                section { class: "shelf",
                    h2 { "New releases" }
                    div { class: "shelf-row",
                        for card in release_cards { {card} }
                    }
                }
                section { class: "shelf",
                    h2 { "Recommended for you" }
                    div { class: "track-list",
                        for row in recommended_rows { {row} }
                    }
                }
            }
        }
    }
}