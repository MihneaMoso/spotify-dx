use dioxus::prelude::*;

use crate::ui::components::AlbumArt;

/// A tappable media card: playlist / album / artist poster with title + subtitle.
/// `extra_class` augments the root class (e.g. `artist-round` for circular art).
#[component]
pub fn MediaCard(
    title: String,
    subtitle: String,
    image_url: String,
    seed: String,
    #[props(default)] extra_class: Option<String>,
    onselect: EventHandler<()>,
) -> Element {
    let root_class = match extra_class.as_deref() {
        Some(extra) => format!("media-card {extra}"),
        None => "media-card".to_string(),
    };
    rsx! {
        div {
            class: "{root_class}",
            onclick: move |_| onselect.call(()),
            AlbumArt { url: image_url, seed: seed, class: Some("media-card-art".to_string()) }
            div { class: "media-card-title", title: "{title}", "{title}" }
            div { class: "media-card-subtitle", title: "{subtitle}", "{subtitle}" }
        }
    }
}