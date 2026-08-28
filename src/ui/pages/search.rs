use dioxus::prelude::*;
use futures::channel::mpsc::UnboundedReceiver;
use futures::stream::StreamExt;

use crate::spotify::api;
use crate::spotify::models::{Album, Artist};
use crate::state::SearchUiState;
use crate::ui::components::{MediaCard, TrackRow};
use crate::ui::router::Route;

/// Debounce window for the query pipeline.
const DEBOUNCE_MS: u64 = 250;

#[component]
pub fn Search() -> Element {
    // Seed from the top-bar search hand-off (consumed once, so back-nav
    // doesn't resurrect stale text).
    let mut query = use_signal(|| std::mem::take(&mut *crate::state::SEARCH_SEED.write()));
    let generation = use_signal(|| 0u64);
    let results = use_signal(SearchUiState::default);
    let albums = use_signal(Vec::<Album>::new);
    let artists = use_signal(Vec::<Artist>::new);
    let error_message = use_signal(String::new);

    // Single worker owns the search pipeline. A generation counter drops late
    // responses so an older query can never overwrite a newer one.
    let sender = {
        let mut generation = generation;
        let mut results = results;
        let mut albums = albums;
        let mut artists = artists;
        let mut error_message = error_message;
        use_coroutine(move |mut rx: UnboundedReceiver<String>| async move {
            let mut last: Option<String> = None;
            while let Some(q) = rx.next().await {
                if last.as_deref() == Some(q.as_str()) {
                    continue;
                }
                last = Some(q.clone());
                let q_trim = q.trim().to_string();
                if q_trim.is_empty() {
                    results.write().reset();
                    albums.write().clear();
                    artists.write().clear();
                    continue;
                }
                generation += 1;
                let gen = *generation.peek();
                match api::search(&q_trim, &["track", "album", "artist"], 12).await {
                    Ok(found) => {
                        if *generation.peek() != gen {
                            continue;
                        }
                        let tracks = found.tracks.map(|p| p.items).unwrap_or_default();
                        *albums.write() = found.albums.map(|p| p.items).unwrap_or_default();
                        *artists.write() = found.artists.map(|p| p.items).unwrap_or_default();
                        results.write().query = q_trim.clone();
                        results.write().tracks = tracks;
                        results.write().has_searched = true;
                        results.write().has_errors = false;
                    }
                    Err(err) => {
                        if *generation.peek() == gen {
                            results.write().query = q_trim;
                            results.write().tracks.clear();
                            results.write().has_errors = true;
                            error_message.set(err.to_string());
                        }
                    }
                }
            }
        })
    };

    let snapshot = results.read().clone();
    let albums_list = albums.read().clone();
    let artists_list = artists.read().clone();

    // Fire a top-bar handed-off query exactly once on mount.
    let mut seed_fired = use_signal(|| false);
    use_effect(move || {
        if *seed_fired.peek() {
            return;
        }
        let q = query.peek().clone();
        if !q.is_empty() {
            seed_fired.set(true);
            sender.send(q);
        }
    });

    let navigator = use_navigator();

    // Top result priority: artist > album > track, materialized as owned data.
    #[allow(dead_code)]
    enum Top {
        Artist { id: String, name: String, img: String },
        Album { id: String, name: String, img: String },
        Track { uri: String, name: String },
    }
    let top: Option<Top> = if let Some(a) = artists_list.first() {
        Some(Top::Artist {
            id: a.id.clone(),
            name: a.name.clone(),
            img: a.images.first().map(|i| i.url.clone()).unwrap_or_default(),
        })
    } else if let Some(al) = albums_list.first() {
        Some(Top::Album {
            id: al.id.clone(),
            name: al.name.clone(),
            img: al.images.first().map(|i| i.url.clone()).unwrap_or_default(),
        })
    } else {
        snapshot.tracks.first().map(|t| Top::Track {
            uri: t.uri.clone(),
            name: t.name.clone(),
        })
    };

    let song_rows: Vec<Element> = snapshot
        .tracks
        .iter()
        .take(6)
        .map(|t| {
            let track = t.clone();
            rsx! {
                TrackRow {
                    key: "{track.id}",
                    track: track,
                    index: None,
                    onplay: crate::player::launch_track,
                }
            }
        })
        .collect();

    // Owned shelf tuples (rsx loops cannot contain `let`).
    let album_cards: Vec<(String, String, String, String)> = albums_list
        .iter()
        .map(|al| {
            (
                al.id.clone(),
                al.name.clone(),
                al.artists.first().map(|x| x.name.clone()).unwrap_or_default(),
                al.images.first().map(|i| i.url.clone()).unwrap_or_default(),
            )
        })
        .collect();
    let artist_cards: Vec<(String, String, String)> = artists_list
        .iter()
        .map(|ar| {
            (
                ar.id.clone(),
                ar.name.clone(),
                ar.images.first().map(|i| i.url.clone()).unwrap_or_default(),
            )
        })
        .collect();

    rsx! {
        div { class: "page search",
            header { class: "page-header",
                h1 { "Search" }
                input {
                    class: "search-input",
                    placeholder: "Artists, tracks, albums…",
                    value: "{query}",
                    autofocus: true,
                    oninput: move |evt| {
                        query.set(evt.value());
                        let q = query();
                        spawn(async move {
                            tokio::time::sleep(std::time::Duration::from_millis(DEBOUNCE_MS)).await;
                            sender.send(q);
                        });
                    },
                }
            }

            if snapshot.has_errors {
                div { class: "error-banner", "{error_message}" }
            } else if snapshot.tracks.is_empty() && albums_list.is_empty() {
                if snapshot.has_searched {
                    div { class: "empty-state", "No results for \u{201c}{snapshot.query}\u{201d}." }
                } else {
                    div { class: "empty-state", "Type above to find music." }
                }
            } else {
                if let Some(top) = top {
                    div { class: "section-header", h2 { "Top result" } }
                    div { class: "top-result",
                        match top {
                            Top::Artist { id, name, img } => rsx! {
                                button { class: "top-card round",
                                    onclick: move |_| { navigator.push(Route::Artist { id: id.clone() }); },
                                    if !img.is_empty() { img { src: "{img}", loading: "lazy" } }
                                    span { class: "top-name", "{name}" }
                                    span { class: "tile-sub", "Artist" }
                                }
                            },
                            Top::Album { id, name, img } => rsx! {
                                button { class: "top-card",
                                    onclick: move |_| { navigator.push(Route::Album { id: id.clone() }); },
                                    if !img.is_empty() { img { src: "{img}", loading: "lazy" } }
                                    span { class: "top-name", "{name}" }
                                    span { class: "tile-sub", "Album" }
                                }
                            },
                            Top::Track { uri, name } => rsx! {
                                button { class: "top-card",
                                    onclick: move |_| crate::player::launch(uri.clone()),
                                    span { class: "top-name", "{name}" }
                                    span { class: "tile-sub", "Song" }
                                }
                            },
                        }
                    }
                }

                if !snapshot.tracks.is_empty() {
                    div { class: "section-header", h2 { "Songs" } }
                    div { class: "track-list", for row in song_rows { {row} } }
                }

                if !albums_list.is_empty() {
                    div { class: "section-header", h2 { "Albums" } }
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

                if !artists_list.is_empty() {
                    div { class: "section-header", h2 { "Artists" } }
                    div { class: "shelf-row",
                        for (rid, name, image) in artist_cards {
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
}


