use dioxus::prelude::*;
use futures::channel::mpsc::UnboundedReceiver;
use futures::stream::StreamExt;

use crate::spotify::api;
use crate::state::SearchUiState;
use crate::ui::components::TrackRow;

/// Search page: queries Spotify (debounced) and previews tracks + albums.
#[component]
pub fn Search() -> Element {
    let mut query = use_signal(String::new);
    let generation = use_signal(|| 0u64);
    let results = use_signal(SearchUiState::default);
    let mut error_message = use_signal(String::new);

    // Single worker owns the search pipeline. A generation counter drops late
    // responses so an older query can never overwrite a newer one.
    let sender = {
        let mut generation = generation;
        let mut results = results;
        use_coroutine(move |mut rx: UnboundedReceiver<String>| async move {
            let mut last: Option<String> = None;
            while let Some(q) = rx.next().await {
                if last.as_deref() == Some(q.as_str()) {
                    continue;
                }
                last = Some(q.clone());
                let search_query = q.trim().to_string();
                if search_query.is_empty() {
                    results.write().reset();
                    continue;
                }
                generation += 1;
                let gen = *generation.peek();
                match api::search_tracks(&search_query, 24).await {
                    Ok(tracks) => {
                        if *generation.peek() == gen {
                            results.write().query = search_query.clone();
                            results.write().tracks = tracks;
                            results.write().has_searched = true;
                            results.write().has_errors = false;
                        }
                    }
                    Err(err) => {
                        if *generation.peek() == gen {
                            results.write().query = search_query.clone();
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

    let rows: Vec<Element> = snapshot
        .tracks
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
        .collect();

    rsx! {
        div { class: "page search",
            header { class: "page-header",
                h1 { "Search" }
                input {
                    class: "search-input",
                    placeholder: "Search artists, tracks, albums…",
                    value: "{query}",
                    oninput: move |evt| {
                        query.set(evt.value());
                        let q = query();
                        dioxus::prelude::spawn(async move {
                            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                            sender.send(q);
                        });
                    },
                }
            }

            if snapshot.has_errors {
                div { class: "error-banner",
                    "{error_message}"
                }
            } else if snapshot.tracks.is_empty() {
                if snapshot.has_searched {
                    div { class: "empty-state", "No results for \u{201c}{snapshot.query}\u{201d}." }
                } else {
                    div { class: "empty-state", "Type above to find music." }
                }
            } else {
                div { class: "search-results",
                    div { class: "result-group-title", "Tracks" }
                    div { class: "track-list",
                        for row in rows { {row} }
                    }
                }
            }
        }
    }
}