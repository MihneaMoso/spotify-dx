use dioxus::prelude::*;

use crate::app_error::AppError;
use crate::spotify::api;
use crate::ui::components::{MediaCard, SectionHeader, SkeletonShelves, TrackTable};
use crate::ui::router::Route;

/// Time-of-day greeting, Spotify-style. Pure so it's testable.
pub(crate) fn greeting(hour: u32) -> &'static str {
    match hour {
        0..=5 => "Good night",
        6..=11 => "Good morning",
        12..=17 => "Good afternoon",
        _ => "Good evening",
    }
}

/// One shortcut tile in the "jump back in" grid.
struct Tile {
    route: Route,
    title: String,
    subtitle: String,
    image: String,
}

/// Landing page: greeting, jump-back-in tiles and shelves.
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

    let hour = chrono::Timelike::hour(&chrono::Local::now());
    let greet = greeting(hour);

    let feed = match resource.read().as_ref() {
        Some(Ok(home)) => Some(home.clone()),
        _ => None,
    };

    // "Jump back in": interleave featured playlists and new releases, cap 8.
    let mut tiles: Vec<Tile> = Vec::new();
    if let Some(home) = &feed {
        for pair in home.featured.iter().zip(home.new_releases.iter()) {
            if tiles.len() >= 8 { break; }
            tiles.push(Tile {
                route: Route::Playlist { id: pair.0.id.clone() },
                title: pair.0.name.clone(),
                subtitle: format!("Playlist · {}", pair.0.owner.display_name.clone().unwrap_or_default()),
                image: pair.0.images.first().map(|i| i.url.clone()).unwrap_or_default(),
            });
            if tiles.len() >= 8 { break; }
            tiles.push(Tile {
                route: Route::Album { id: pair.1.id.clone() },
                title: pair.1.name.clone(),
                subtitle: format!("Album · {}", pair.1.artists.first().map(|x| x.name.clone()).unwrap_or_default()),
                image: pair.1.images.first().map(|i| i.url.clone()).unwrap_or_default(),
            });
        }
    }

    // Owned card tuples so event closures are 'static (no borrows of `feed`).
    let featured_cards: Vec<(Route, String, String, String)> = feed
        .as_ref()
        .map(|h| {
            h.featured
                .iter()
                .map(|p| {
                    (
                        Route::Playlist { id: p.id.clone() },
                        p.name.clone(),
                        format!("{} tracks", p.tracks.total),
                        p.images.first().map(|i| i.url.clone()).unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let release_cards: Vec<(Route, String, String, String)> = feed
        .as_ref()
        .map(|h| {
            h.new_releases
                .iter()
                .map(|a| {
                    (
                        Route::Album { id: a.id.clone() },
                        a.name.clone(),
                        a.artists.first().map(|x| x.name.clone()).unwrap_or_default(),
                        a.images.first().map(|i| i.url.clone()).unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let recommended = feed
        .as_ref()
        .map(|h| h.recommended.clone())
        .unwrap_or_default();

    let greet_header = rsx! {
        header { class: "page-header", h1 { "{greet}" } }
    };

    // Compute each state's body in plain Rust (no control-flow-in-rsx).
    let body: Element = if resource.read().is_none() {
        rsx! { SkeletonShelves { count: 3 } }
    } else if let Some(Err(err)) = resource.read().as_ref() {
        rsx! {
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
        }
    } else {
        rsx! {
            if !tiles.is_empty() {
                div { class: "tile-grid",
                    for tile in tiles {
                        button {
                            class: "tile",
                            title: "{tile.title}",
                            onclick: move |_| { navigator.push(tile.route.clone()); },
                            if tile.image.is_empty() {
                                div { class: "tile-art placeholder-tile" }
                            } else {
                                img { class: "tile-art", src: "{tile.image}", loading: "lazy" }
                            }
                            span { class: "tile-text",
                                span { class: "tile-title", "{tile.title}" }
                                span { class: "tile-sub", "{tile.subtitle}" }
                            }
                        }
                    }
                }
            }

            SectionHeader { title: "Featured playlists".to_string() }
            div { class: "shelf-row",
                for (route, title, subtitle, image) in featured_cards {
                    MediaCard {
                        key: "{title}-{image}",
                        title: title.clone(),
                        subtitle: subtitle.clone(),
                        image_url: image.clone(),
                        seed: title.clone(),
                        onselect: move |_| { navigator.push(route.clone()); },
                    }
                }
            }

            SectionHeader { title: "New releases".to_string() }
            div { class: "shelf-row",
                for (route, title, subtitle, image) in release_cards {
                    MediaCard {
                        key: "{title}-{image}",
                        title: title.clone(),
                        subtitle: subtitle.clone(),
                        image_url: image.clone(),
                        seed: title.clone(),
                        onselect: move |_| { navigator.push(route.clone()); },
                    }
                }
            }

            SectionHeader { title: "Recommended for you".to_string() }
            TrackTable { tracks: recommended, numbered: false }
        }
    };

    rsx! {
        div { class: "page home",
            {greet_header}
            {body}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::greeting;

    #[test]
    fn greetings_cover_the_whole_day() {
        assert_eq!(greeting(0), "Good night");
        assert_eq!(greeting(3), "Good night");
        assert_eq!(greeting(6), "Good morning");
        assert_eq!(greeting(11), "Good morning");
        assert_eq!(greeting(12), "Good afternoon");
        assert_eq!(greeting(17), "Good afternoon");
        assert_eq!(greeting(18), "Good evening");
        assert_eq!(greeting(23), "Good evening");
    }
}
