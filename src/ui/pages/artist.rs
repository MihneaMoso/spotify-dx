use dioxus::prelude::*;

use crate::spotify::api;
use crate::ui::components::{HeroHeader, MediaCard, SectionHeader, TrackTable};
use crate::ui::router::Route;

/// Artist page: hero, expandable popular tracks, discography + related shelves.
#[component]
pub fn Artist(id: String) -> Element {
    let navigator = use_navigator();

    let resource = use_resource(move || {
        let id = id.clone();
        async move { api::get_artist_page(&id).await }
    });

    // Clone the Ok payload out so no read guard crosses the rsx boundary.
    let loaded = resource
        .read()
        .as_ref()
        .and_then(|r| r.as_ref().ok())
        .cloned();
    let is_err = matches!(resource.read().as_ref(), Some(Err(_)));

    let Some(page) = loaded else {
        return if is_err {
            rsx! {
                div { class: "page detail",
                    div { class: "error-banner", "Artist unavailable." }
                }
            }
        } else {
            rsx! {
                div { class: "page detail",
                    div { class: "page-spinner", div { class: "spinner" } }
                }
            }
        };
    };
    let artist = page.artist;
    let top = page.top_tracks;
    let albums_owned = page.albums;
    let related_owned = page.related;

    let mut expanded = use_signal(|| false);
    let name = artist.name.clone();
    let followers = artist.followers.total;
    let genres = artist.genres.join(" · ");
    let meta = format!("{} followers · {}", format_count(followers), genres);
    let art_url = artist.images.first().map(|i| i.url.clone()).unwrap_or_default();
    let seed = artist.id.clone();
    let shown = if *expanded.peek() {
        top.clone()
    } else {
        top.iter().take(5).cloned().collect()
    };
    let top_track = top.first().cloned();
    let shuffle_pool = top.clone();
    let top_len = top.len();

    // Precompute owned shelf tuples (no `let` allowed inside rsx loops).
    let album_cards: Vec<(String, String, String, String)> = albums_owned
        .into_iter()
        .map(|al| {
            let img = al.images.first().map(|i| i.url.clone()).unwrap_or_default();
            let year = al.release_date.split('-').next().unwrap_or("").to_string();
            (al.id.clone(), al.name.clone(), year, img)
        })
        .collect();
    let related_cards: Vec<(String, String, String)> = related_owned
        .into_iter()
        .map(|ar| {
            let img = ar.images.first().map(|i| i.url.clone()).unwrap_or_default();
            (ar.id.clone(), ar.name.clone(), img)
        })
        .collect();

    rsx! {
        div { class: "page detail",
            HeroHeader {
                kind: "Artist".to_string(),
                title: name,
                meta: meta,
                image_url: art_url,
                seed: seed,
                onplay: move |_| {
                    if let Some(t) = top_track.clone() {
                        crate::player::launch_track(t);
                    }
                },
                onshuffle: move |_| {
                    if !shuffle_pool.is_empty() {
                        let i = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.subsec_nanos() as usize)
                            .unwrap_or(0);
                        crate::player::launch_track(shuffle_pool[i % shuffle_pool.len()].clone());
                    }
                },
            }

            SectionHeader { title: "Popular".to_string() }
            TrackTable { tracks: shown, numbered: true }
            if top_len > 5 {
                button {
                    class: "show-more",
                    onclick: move |_| expanded.toggle(),
                    if *expanded.read() { "Show less" } else { "Show all {top_len}" }
                }
            }

            if !album_cards.is_empty() {
                SectionHeader { title: "Discography".to_string() }
                div { class: "shelf-row",
                    for (aid, name, subtitle, image) in album_cards {
                        MediaCard {
                            key: "{aid}",
                            title: name.clone(),
                            subtitle: subtitle.clone(),
                            image_url: image.clone(),
                            seed: aid.clone(),
                            onselect: move |_| { navigator.push(Route::Album { id: aid.clone() }); },
                        }
                    }
                }
            }

            if !related_cards.is_empty() {
                SectionHeader { title: "Fans also like".to_string() }
                div { class: "shelf-row",
                    for (rid, name, image) in related_cards {
                        MediaCard {
                            key: "{rid}",
                            extra_class: Some("artist-round".to_string()),
                            title: name.clone(),
                            subtitle: "Artist".to_string(),
                            image_url: image.clone(),
                            seed: rid.clone(),
                            onselect: move |_| { navigator.push(Route::Artist { id: rid.clone() }); },
                        }
                    }
                }
            }
        }
    }
}

/// Compact follower count ("1.2M").
fn format_count(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.1}K", n as f64 / 1_000.0),
        _ => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
}

#[cfg(test)]
mod tests {
    use super::format_count;

    #[test]
    fn follower_counts_compact() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(940), "940");
        assert_eq!(format_count(12_400), "12.4K");
        assert_eq!(format_count(3_452_901), "3.5M");
    }
}
