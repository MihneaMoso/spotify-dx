//! Now-playing right column (≥1280 px): large artwork, track metadata and a
//! live position readout. Pure view over `PLAYER_STATE` — hidden entirely by
//! CSS below 1280 px; the toggle in the player bar flips `SHOW_NOW_PLAYING`.

use dioxus::prelude::*;

use crate::state::{PLAYER_STATE, SHOW_NOW_PLAYING, format_duration};
use crate::ui::components::AlbumArt;
use crate::ui::icons::back_arrow;

#[component]
pub fn NowPlayingView() -> Element {
    let state = PLAYER_STATE.read();
    let show = *SHOW_NOW_PLAYING.read();

    // Hidden by user toggle: render nothing so the grid cell stays empty.
    if !show {
        return rsx! {};
    }

    let Some(track) = state.track.as_ref() else {
        return rsx! {
            aside { class: "now-playing-col",
                div { class: "np-header",
                    span { class: "np-label", "Now playing" }
                }
                div { class: "np-empty",
                    "Nothing is playing yet. Pick a track and it will show up here."
                }
            }
        };
    };

    let title = track.name.clone();
    let artists = state.subtitle();
    let album = track.album.name.clone();
    let art_url = state.large_art_url();
    let seed = track.id.clone();

    let position = format_duration(state.position_ms);
    let duration = format_duration(state.duration_ms);

    rsx! {
        aside { class: "now-playing-col",
            div { class: "np-header",
                span { class: "np-label", "Now playing" }
                button {
                    class: "tb-btn",
                    title: "Close panel",
                    onclick: move |_| *SHOW_NOW_PLAYING.write() = false,
                    {back_arrow(16)}
                }
            }
            div { class: "np-art",
                AlbumArt { url: art_url, seed: seed, class: None }
            }
            div {
                div { class: "np-title", "{title}" }
                div { class: "np-artists", "{artists}" }
                a {
                    class: "np-album",
                    href: "#",
                    onclick: move |evt| evt.stop_propagation(),
                    "{album}"
                }
            }
            div { class: "np-meta-row",
                span { class: "np-position", "{position}" }
                span { class: "np-duration", "{duration}" }
            }
        }
    }
}