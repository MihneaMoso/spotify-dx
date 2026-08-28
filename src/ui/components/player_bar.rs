use dioxus::prelude::*;
use futures::channel::mpsc::UnboundedReceiver;

use crate::player;
use crate::state::{PLAYER_STATE, SHOW_NOW_PLAYING, RepeatMode};
use crate::ui::components::{AlbumArt, ProgressBar, VolumeBar};

/// Persistent bottom bar: artwork, controls, progress and volume. Always lives
/// inside the app shell, so it stays mounted across route changes.
#[component]
pub fn PlayerBar() -> Element {
    use_coroutine(|_rx: UnboundedReceiver<()>| async move {
        // Keep the clock ticking while playing. On the open engine the sink
        // thread publishes the real position, so only fake-advance the SDK path.
        let mut ticker = tokio::time::interval(std::time::Duration::from_millis(250));
        loop {
            ticker.tick().await;
            if PLAYER_STATE.peek().is_playing && !crate::player::is_open_engine() {
                PLAYER_STATE.write().position_ms =
                    PLAYER_STATE.peek().position_ms.saturating_add(250);
            }
        }
    });

    let playing = PLAYER_STATE.read().is_playing;
    let volume = PLAYER_STATE.read().volume;
    let shuffle = PLAYER_STATE.read().shuffle;
    let repeat = PLAYER_STATE.read().repeat;
    let liked = PLAYER_STATE.read().liked;

    let state = PLAYER_STATE.peek();
    let (art_url, seed, title, subtitle, pos, dur) = match &state.track {
        Some(t) => (
            t.album
                .images
                .iter()
                .find(|img| img.width.is_some() && img.width.unwrap_or(0) >= 64)
                .or_else(|| t.album.images.first())
                .map(|img| img.url.clone())
                .unwrap_or_default(),
            t.id.clone(),
            t.name.clone(),
            state.subtitle(),
            state.position_ms,
            state.duration_ms,
        ),
        None => (String::new(), String::new(), String::new(), String::new(), 0, 0),
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
                            let next = !PLAYER_STATE.peek().shuffle;
                            PLAYER_STATE.write().shuffle = next;
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
                        title: "Repeat: {repeat_label(repeat)}",
                        class: if repeat != RepeatMode::Off { "ctrl active" } else { "ctrl" },
                        onclick: move |_| {
                            PLAYER_STATE.write().repeat = match PLAYER_STATE.peek().repeat {
                                RepeatMode::Off => RepeatMode::Context,
                                RepeatMode::Context => RepeatMode::Track,
                                RepeatMode::Track => RepeatMode::Off,
                            };
                        },
                        {crate::ui::icons::repeat(18, repeat != RepeatMode::Off)}
                    }
                }
                ProgressBar {
                    position_ms: pos,
                    duration_ms: dur,
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

fn repeat_label(repeat: RepeatMode) -> &'static str {
    match repeat {
        RepeatMode::Off => "Repeat off",
        RepeatMode::Context => "Repeat all",
        RepeatMode::Track => "Repeat one",
    }
}