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

/// Landing page: greeting, user playlists, liked tracks.
#[component]
pub fn Home() -> Element {
    let navigator = use_navigator();
    let mut retry_count = use_signal(|| 0u32);
    let mut retry_pending = use_signal(|| false);

    let resource = use_resource(move || {
        let attempt = *retry_count.read();
        tracing::info!("home: fetching feed (attempt {attempt})");
        async move { api::get_home().await }
    });

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

    let playlist_cards: Vec<(Route, String, String, String)> = feed
        .as_ref()
        .map(|h| {
            h.playlists
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

    let liked_tracks = feed
        .as_ref()
        .map(|h| h.liked_tracks.clone())
        .unwrap_or_default();

    let greet_header = rsx! {
        header { class: "page-header", h1 { "{greet}" } }
    };

    let body: Element = if resource.read().is_none() {
        rsx! { SkeletonShelves { count: 2 } }
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
            if !playlist_cards.is_empty() {
                SectionHeader { title: "Your playlists".to_string() }
                div { class: "shelf-row",
                    for (route, title, subtitle, image) in playlist_cards {
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
            }

            if !liked_tracks.is_empty() {
                SectionHeader { title: "Liked songs".to_string() }
                TrackTable { tracks: liked_tracks, numbered: false }
            }
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
