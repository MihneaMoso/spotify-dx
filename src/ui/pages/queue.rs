//! Play queue: what's queued locally, with clear/remove. The queue model is
//! populated by Phase 4's local-queue work; until then this renders whatever
//! exists (usually just the current track context).

use dioxus::prelude::*;

use crate::state::{format_duration, PLAYER_STATE};
use crate::ui::components::TrackRow;

#[component]
pub fn Queue() -> Element {
    let state = PLAYER_STATE.read();
    let current = state.track.clone();
    let queue = state.queue.clone();

    let has_queue = !queue.is_empty();

    // Per-id occurrence counts so duplicate queue entries still key
    // uniquely (stable identity without the row index).
    let mut occurrences: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let rows: Vec<(String, u32, crate::spotify::models::Track)> = queue
        .into_iter()
        .enumerate()
        .map(|(i, t)| {
            let n = occurrences.entry(t.id.clone()).or_insert(0);
            *n += 1;
            let key = if *n > 1 {
                format!("{}-{}", t.id, n)
            } else {
                t.id.clone()
            };
            (key, (i + 1) as u32, t)
        })
        .collect();

    rsx! {
        div { class: "page detail",
            header { class: "page-header queue-header",
                h1 { "Queue" }
                if has_queue {
                    button {
                        class: "ghost",
                        onclick: move |_| PLAYER_STATE.write().queue.clear(),
                        "Clear queue"
                    }
                }
            }

            if let Some(track) = &current {
                div { class: "section-header", h2 { "Now playing" } }
                div { class: "queue-now",
                    span { class: "queue-now-title", "{track.name}" }
                    span { class: "np-artists", "{state.subtitle()}" }
                    QueuePosition {}
                }
            }

            div { class: "section-header",
                h2 { if has_queue { "Next up" } else { "Next up · empty" } }
            }
            if has_queue {
                div { class: "track-list",
                    for (key, n, t) in rows {
                        TrackRow {
                            // Stable identity (id, not index) with numbered
                            // display kept: edits move rows instead of
                            // remounting everything below them.
                            key: "{key}",
                            track: t,
                            index: Some(n),
                            onplay: crate::player::launch_track,
                        }
                    }
                }
            } else {
                div { class: "empty-state",
                    "Your queue is empty. Tracks you enqueue will wait here."
                }
            }
        }
    }
}

/// Live position readout, isolated so the 4Hz ticks re-render only this
/// text node instead of the whole queue page (rows keep stable keys and
/// bail out of diffing on equal props).
#[component]
fn QueuePosition() -> Element {
    let position = format_duration(PLAYER_STATE.read().position_ms);
    let duration = format_duration(PLAYER_STATE.read().duration_ms);
    rsx! {
        span { class: "np-position", "{position} / {duration}" }
    }
}
