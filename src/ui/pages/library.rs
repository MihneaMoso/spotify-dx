use dioxus::prelude::*;

use crate::spotify::api;
use crate::spotify::models::{Album, Playlist};
use crate::ui::components::MediaCard;
use crate::ui::router::Route;

/// Case-insensitive substring match used by the library filter.
fn matches(name: &str, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    q.is_empty() || name.to_lowercase().contains(&q)
}

/// Library view filter chips.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    All,
    Playlists,
    Albums,
    Liked,
}

impl Tab {
    const ALL: [Tab; 4] = [Tab::All, Tab::Playlists, Tab::Albums, Tab::Liked];
    fn label(self) -> &'static str {
        match self {
            Tab::All => "All",
            Tab::Playlists => "Playlists",
            Tab::Albums => "Albums",
            Tab::Liked => "Liked",
        }
    }
}

#[component]
pub fn Library() -> Element {
    let navigator = use_navigator();
    let mut tab = use_signal(|| Tab::All);
    let mut filter = use_signal(String::new);
    let mut alpha = use_signal(|| false);

    let resource = use_resource(|| async move {
        let playlists = api::get_user_playlists().await;
        let albums = api::get_user_albums(50, 0).await;
        let liked = api::get_user_saved_tracks(50, 0).await;
        (playlists, albums, liked)
    });

    let (is_err, playlists, albums, liked) = {
        let snap = resource.read();
        match snap.as_ref() {
            None => (false, Vec::new(), Vec::new(), Vec::new()),
            Some((p, a, l)) => {
                let err = p.is_err() || a.is_err() || l.is_err();
                (
                    err,
                    p.as_ref().cloned().unwrap_or_default(),
                    a.as_ref().cloned().unwrap_or_default(),
                    l.as_ref().map(|page| page.items.clone()).unwrap_or_default(),
                )
            }
        }
    };
    let q = filter.read().clone();
    let sort_alpha = *alpha.read();

    let mut pl: Vec<&Playlist> = playlists.iter().filter(|p| matches(&p.name, &q)).collect();
    let mut al: Vec<&Album> = albums.iter().filter(|a| matches(&a.name, &q)).collect();
    if sort_alpha {
        pl.sort_by_key(|p| p.name.to_lowercase());
        al.sort_by_key(|a| a.name.to_lowercase());
    }

    let show_playlists = matches!(*tab.read(), Tab::All | Tab::Playlists);
    let show_albums = matches!(*tab.read(), Tab::All | Tab::Albums);
    let show_liked = matches!(*tab.read(), Tab::All | Tab::Liked);

// Owned card tuples so closures are 'static.
let playlist_cards: Vec<(String, String, String, String)> = pl
    .into_iter()
    .map(|p| {
        (
            p.id.clone(),
            p.name.clone(),
            format!("by {}", p.owner.display_name.clone().unwrap_or_default()),
            p.images.first().map(|i| i.url.clone()).unwrap_or_default(),
        )
    })
    .collect();
let album_cards: Vec<(String, String, String, String)> = al
    .into_iter()
    .map(|a| {
        (
            a.id.clone(),
            a.name.clone(),
            a.artists.first().map(|x| x.name.clone()).unwrap_or_default(),
            a.images.first().map(|i| i.url.clone()).unwrap_or_default(),
        )
    })
    .collect();
// Playable liked tracks as owned (id, track) rows.
let playable_rows: Vec<(String, crate::spotify::models::Track)> = liked
    .iter()
    .filter_map(|s| s.playable().cloned())
    .map(|t| (t.id.clone(), t))
    .collect();

    rsx! {
        div { class: "page library",
            header { class: "page-header",
                h1 { "Your Library" }
                div { class: "library-tools",
                    input {
                        class: "search-input lib-filter",
                        r#type: "search",
                        placeholder: "Filter…",
                        value: "{q}",
                        oninput: move |evt| filter.set(evt.value()),
                    }
                    button {
                        class: if sort_alpha { "chip active" } else { "chip" },
                        title: "Toggle A–Z sorting",
                        onclick: move |_| {
                            let cur = *alpha.peek();
                            alpha.set(!cur);
                        },
                        "A–Z"
                    }
                }
            }

            div { class: "chip-row",
                for t in Tab::ALL {
                    button {
                        class: if *tab.read() == t { "chip active" } else { "chip" },
                        onclick: move |_| tab.set(t),
                        "{t.label()}"
                    }
                }
            }

            if resource.read().is_none() {
                div { class: "page-spinner", div { class: "spinner" } }
            } else if is_err {
                div { class: "error-banner",
                    "Couldn't load your library. Make sure you're signed in and online."
                }
            } else {
                if show_playlists && !playlist_cards.is_empty() {
                    div { class: "section-header", h2 { "Playlists" } }
                    div { class: "card-grid",
                        for (pid, name, subtitle, image) in playlist_cards {
                            MediaCard {
                                key: "{pid}",
                                title: name.clone(),
                                subtitle: subtitle.clone(),
                                image_url: image.clone(),
                                seed: pid.clone(),
                                onselect: move |_| { navigator.push(Route::Playlist { id: pid.clone() }); },
                            }
                        }
                    }
                }

                if show_albums && !album_cards.is_empty() {
                    div { class: "section-header", h2 { "Albums" } }
                    div { class: "card-grid",
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

                if show_liked {
                    div { class: "section-header", h2 { "Liked songs" } }
                    div { class: "track-list",
                        if playable_rows.is_empty() {
                            div { class: "empty-state", "Nothing liked yet." }
                        }
                        for (kid, t) in playable_rows {
                            TrackRowLite {
                                key: "{kid}",
                                track: t,
                                index_by_id: kid,
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Minimal list row for the library's liked section.
#[component]
fn TrackRowLite(track: crate::spotify::models::Track, index_by_id: String) -> Element {
    let uri = track.uri.clone();
    rsx! {
        button {
            class: "lib-row",
            onclick: move |_| crate::player::launch(uri.clone()),
            span { class: "track-index", "{index_by_id}" }
            span { class: "lib-row-name", "{track.name}" }
            span { class: "np-duration", "{crate::state::format_duration(track.duration_ms)}" }
        }
    }
}
