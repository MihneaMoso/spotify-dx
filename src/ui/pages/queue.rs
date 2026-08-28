//! Play queue: what's queued locally, with clear/remove. The queue model is
//! populated by Phase 4's local-queue work; until then this renders whatever
//! exists (usually just the current track context).

use dioxus::prelude::*;

use crate::state::{PLAYER_STATE, format_duration};
use crate::ui::components::TrackRow;

#[component]
pub fn Queue() -> Element {
    let state = PLAYER_STATE.read();
    let current = state.track.clone();
    let queue = state.queue.clone();
    let position = format_duration(state.position_ms);
    let duration = format_duration(state.duration_ms);

    let has_queue = !queue.is_empty();

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
                    span { class: "np-position", "{position} / {duration}" }
                }
            }

            div { class: "section-header",
                h2 { if has_queue { "Next up" } else { "Next up · empty" } }
            }
            if has_queue {
                div { class: "track-list",
                    for (i, t) in queue.into_iter().enumerate() {
                        TrackRow {
                            key: "{t.id}-{i}",
                            track: t,
                            index: Some((i + 1) as u32),
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