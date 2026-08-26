//! Shared page primitives: section headers, detail-page heroes, track tables
//! with progressive reveal, and loading skeletons.
//!
//! Everything here is pure presentation over props/signals — no fetching.

use dioxus::prelude::*;

use crate::state::format_duration;
use crate::ui::components::{AlbumArt, TrackRow};
use crate::ui::icons::{heart, play, shuffle};

/// Shelf heading with an optional trailing action ("Show all").
#[component]
pub fn SectionHeader(title: String, action: Option<String>) -> Element {
    rsx! {
        div { class: "section-header",
            h2 { "{title}" }
            if let Some(action) = action {
                span { class: "section-action", "{action}" }
            }
        }
    }
}

/// Gradient hero used by Playlist / Album / Artist / Liked detail pages.
#[component]
pub fn HeroHeader(
    kind: String,
    title: String,
    meta: String,
    image_url: String,
    seed: String,
    onplay: EventHandler<()>,
    onshuffle: EventHandler<()>,
) -> Element {
    rsx! {
        div { class: "detail-hero",
            if image_url.is_empty() {
                div { class: "detail-art placeholder-hero" }
            } else {
                div { class: "detail-art",
                    AlbumArt { url: image_url, seed: seed, class: None }
                }
            }
            div { class: "detail-text",
                span { class: "detail-kind", "{kind}" }
                h1 { "{title}" }
                p { class: "detail-subtitle", "{meta}" }
            }
            div { class: "hero-actions",
                button {
                    class: "play-fab",
                    title: "Play",
                    onclick: move |_| onplay(()),
                    {play(24)}
                }
                button {
                    class: "tb-btn",
                    title: "Shuffle",
                    onclick: move |_| onshuffle(()),
                    {shuffle(20, false)}
                }
                span { class: "hero-heart", title: "Likes arrive with Phase 4",
                    {heart(18, false)}
                }
            }
        }
    }
}

/// How many rows a [`TrackTable`] shows before offering to expand.
const TABLE_CHUNK: usize = 60;

/// Track list with progressive reveal: renders [`TABLE_CHUNK`] rows up front
/// and grows in chunks via an inline "Show all" affordance. True windowed
/// virtualization lands with the Phase-4 store work if lists grow past a few
/// hundred rows; this keeps first paint cheap without scroll plumbing.
#[component]
pub fn TrackTable(tracks: Vec<crate::spotify::models::Track>, numbered: bool) -> Element {
    let mut visible = use_signal(|| TABLE_CHUNK);

    let total = tracks.len();
    let shown = (*visible.peek()).min(total);

    let rows: Vec<Element> = tracks
        .iter()
        .take(shown)
        .enumerate()
        .map(|(i, t)| {
            let index = numbered.then_some(i as u32 + 1);
            let track = t.clone();
            let uri = track.uri.clone();
            rsx! {
                TrackRow {
                    key: "{track.id}-{i}",
                    track: track,
                    index: index,
                    onplay: move |_| crate::player::launch(uri.clone()),
                }
            }
        })
        .collect();

    rsx! {
        div { class: "track-list",
            for row in rows { {row} }
            if shown < total {
                button {
                    class: "show-more",
                    onclick: move |_| *visible.write() += TABLE_CHUNK * 2,
                    "Show more ({total - shown} hidden)"
                }
            }
        }
    }
}

/// Compact duration chip used in tables/meta lines.
#[component]
pub fn Duration(ms: u64) -> Element {
    rsx! { span { class: "np-duration", "{format_duration(ms)}" } }
}

/// Shimmering placeholders matching the shelf layout, shown while fetching.
#[component]
pub fn SkeletonShelves(count: u32) -> Element {
    rsx! {
        for _ in 0..count {
            section { class: "shelf",
                div { class: "skeleton skeleton-line" }
                div { class: "shelf-row",
                    for _ in 0..6 {
                        div { class: "skeleton skeleton-card" }
                    }
                }
            }
        }
    }
}