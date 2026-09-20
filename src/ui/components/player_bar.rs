use dioxus::prelude::*;
use futures::channel::mpsc::UnboundedReceiver;

use crate::player;
use crate::state::{PLAYER_STATE, SHOW_NOW_PLAYING};
use crate::ui::components::{AlbumArt, ProgressBar, VolumeBar};

/// Persistent bottom bar: artwork, controls, progress and volume. Always lives
/// inside the app shell, so it stays mounted across route changes.
#[component]
pub fn PlayerBar() -> Element {
    // On the open engine the sink + shared position task publish the real
    // position, so this fake-advance clock is only needed on the SDK path.
    // The hook itself is unconditional (hooks must run on every render); the
    // open-engine case parks it forever so there are zero timer wakeups while
    // idle. On the SDK path it ticks at 250ms only while actually advancing
    // the clock and sleeps at 1s when paused, with a compare-before-write so
    // a steady state never re-renders subscribers.
    use_coroutine(|_rx: UnboundedReceiver<()>| async move {
        if crate::player::is_open_engine() {
            futures::future::pending::<()>().await;
            return;
        }
        loop {
            let playing = PLAYER_STATE.peek().is_playing;
            tokio::time::sleep(std::time::Duration::from_millis(if playing {
                250
            } else {
                1000
            }))
            .await;
            if !playing {
                continue;
            }
            // Single non-re-entrant write: take the current position first,
            // then write, so the write guard is never alive during the read
            // (writing and peeking in one expression re-borrows the same
            // `GlobalSignal` and panics with AlreadyBorrowed).
            let pos = PLAYER_STATE.peek().position_ms;
            let dur = PLAYER_STATE.peek().duration_ms;
            let next = if dur == 0 {
                pos.saturating_add(250)
            } else {
                pos.saturating_add(250).min(dur)
            };
            if next != pos {
                PLAYER_STATE.write().position_ms = next;
            }
        }
    });

    let playing = PLAYER_STATE.read().is_playing;
    let volume = PLAYER_STATE.read().volume;
    let shuffle = PLAYER_STATE.read().shuffle;
    let liked = PLAYER_STATE.read().liked;

    // Subscribed snapshot (not peek): track/art/position must update on
    // track changes that leave is_playing untouched (auto-advance while
    // playing froze the bar until some other field flipped).
    let state = PLAYER_STATE.read().clone();
    let (art_url, seed, title, subtitle, pos, dur) = match &state.track {
        Some(t) => (
            crate::ui::components::pick_artwork(&t.album.images, 64),
            t.id.clone(),
            t.name.clone(),
            state.subtitle(),
            state.position_ms,
            state.duration_ms,
        ),
        None => (
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            0,
            0,
        ),
    };

    let _ = playing; // toggling is state-driven from the SDK / connect API.

    let bg_style = if art_url.is_empty() {
        String::new()
    } else {
        format!("background-image: url(\"{art_url}\");")
    };

    rsx! {
        footer { class: "player-bar",
            div { class: "player-bg", style: "{bg_style}" }
            div { class: "player-bg-overlay" }
            div { class: "player-left",
                AlbumArt { url: art_url, seed: seed, class: Some("player-art".to_string()) }
                div { class: "player-meta",
                    div { class: "player-title", title: "{title}", "{title}" }
                    div { class: "player-artists", title: "{subtitle}", "{subtitle}" }
                }
            }
            div { class: "player-center",
                div { class: "player-controls",
                    button {
                        title: "Shuffle",
                        class: if shuffle { "ctrl active" } else { "ctrl" },
                        onclick: move |_| {
                            // Real engine path (queue reorder + snapshot),
                            // not a local-only flip: the old handler set the
                            // bool while the queue order never changed.
                            let next = !PLAYER_STATE.peek().shuffle;
                            PLAYER_STATE.write().set_shuffle(next);
                        },
                        {crate::ui::icons::shuffle(18, shuffle)}
                    }
                    button { class: "ctrl", title: "Previous",
                        onclick: move |_| { dioxus::prelude::spawn(async { let _ = player::prev().await; }); },
                        {crate::ui::icons::skip_back(22)}
                    }
                    button {
                        class: "ctrl big", title: if playing { "Pause" } else { "Play" },
                        onclick: move |_| {
                            dioxus::prelude::spawn(async {
                                if PLAYER_STATE.peek().is_playing {
                                    let _ = player::pause().await;
                                } else {
                                    let _ = player::play().await;
                                }
                            });
                        },
                        {if playing {
                            crate::ui::icons::pause(26)
                        } else {
                            crate::ui::icons::play(26)
                        }}
                    }
                    button { class: "ctrl", title: "Next",
                        onclick: move |_| { dioxus::prelude::spawn(async { let _ = player::next().await; }); },
                        {crate::ui::icons::skip_forward(22)}
                    }
                    button {
                        // Disabled, honestly: no engine path reads repeat
                        // state (neither the SDK relay nor the open sink
                        // loops on it), so cycling it only lied in the UI.
                        // Re-enable with a real loop when a backend lands.
                        title: "Repeat (not available yet)",
                        class: "ctrl",
                        disabled: true,
                        {crate::ui::icons::repeat(18, false)}
                    }
                }
                ProgressBar {
                    position_ms: pos,
                    duration_ms: dur,
                    onpreview: move |ms| {
                        PLAYER_STATE.write().position_ms = ms;
                    },
                    onscrub: move |ms| {
                        PLAYER_STATE.write().position_ms = ms;
                        dioxus::prelude::spawn(async move {
                            let _ = player::seek(ms).await;
                        });
                    },
                }
            }
            div { class: "player-right",
                button {
                    class: "ctrl",
                    title: "Queue",
                    onclick: move |_| {
                        let next = !*SHOW_NOW_PLAYING.peek();
                        *SHOW_NOW_PLAYING.write() = next;
                    },
                    {crate::ui::icons::queue_list(18)}
                }
                button {
                    title: if liked { "Remove from Liked Songs" } else { "Save to Liked Songs" },
                    class: if liked { "ctrl active" } else { "ctrl" },
                    onclick: move |_| {
                        // Optimistic stub; the real /me/tracks call lands in Phase 4.
                        let next = !PLAYER_STATE.peek().liked;
                        PLAYER_STATE.write().liked = next;
                    },
                    {crate::ui::icons::heart(18, liked)}
                }
                {crate::ui::icons::volume(18)}
                VolumeBar {
                    volume: volume,
                    onvolume: move |v| {
                        PLAYER_STATE.write().volume = v;
                        dioxus::prelude::spawn(async move {
                            let _ = player::volume(v).await;
                        });
                    },
                }
            }
        }
    }
}
