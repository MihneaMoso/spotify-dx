use dioxus::prelude::*;

use crate::app_error::AppError;
use crate::spotify::api;
use crate::ui::components::{MediaCard, TrackRow};
use crate::ui::router::Route;

/// Landing page: featured playlists, new releases and a recommended track list.
#[component]
pub fn Home() -> Element {
    let navigator = use_navigator();
    // Bump to re-run the feed fetch. The resource re-runs when this changes,
    // giving the rate-limited case a self-healing retry loop.
    let mut retry_count = use_signal(|| 0u32);
    let mut retry_pending = use_signal(|| false);

    let resource = use_resource(move || {
        let attempt = *retry_count.read();
        tracing::info!("home: fetching feed (attempt {attempt})");
        async move { api::get_home().await }
    });

    // api.spotify.com rate-limiting isn't a login problem — retry on a slow
    // timer so the feed loads by itself once the quota clears.
    use_effect(move || {
        let rate_limited = matches!(resource.read().as_ref(), Some(Err(AppError::RateLimited)));
        if rate_limited && !*retry_pending.read() {
            retry_pending.set(true);
            spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                *retry_count.write() += 1;
                retry_pending.set(false);
            });
        }
    });

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
                        if matches!(err, AppError::RateLimited) {
                            "Spotify's API is temporarily limiting requests. The feed will retry automatically."
                        } else {
                            "Couldn't load your feed. Make sure you're signed in and online."
                        }
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