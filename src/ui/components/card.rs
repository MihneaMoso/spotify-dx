use dioxus::prelude::*;

use crate::ui::components::AlbumArt;

/// A tappable media card: playlist / album / artist poster with title + subtitle.
#[component]
pub fn MediaCard(
    title: String,
    subtitle: String,
    image_url: String,
    seed: String,
    onselect: EventHandler<()>,
) -> Element {
    rsx! {
        div {
            class: "media-card",
            onclick: move |_| onselect.call(()),
            AlbumArt { url: image_url, seed: seed, class: Some("media-card-art".to_string()) }
            div { class: "media-card-title", title: "{title}", "{title}" }
            div { class: "media-card-subtitle", title: "{subtitle}", "{subtitle}" }
        }
    }
}