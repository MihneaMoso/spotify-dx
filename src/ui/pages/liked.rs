//! Liked Songs: paginated `/me/tracks` with incremental "load more".

use dioxus::prelude::*;

use crate::spotify::api;
use crate::spotify::models::SavedTrack;
use crate::ui::components::{HeroHeader, TrackRow};
use crate::ui::icons::heart;

const PAGE_SIZE: u32 = 50;

/// Captures the pagination signals so the fetch helper can be a plain `fn`
/// (no closure-capture headaches with dioxus effects/buttons).
#[derive(Clone)]
struct FetchCtx {
    items: Signal<Vec<SavedTrack>>,
    total: Signal<u32>,
    loading: Signal<bool>,
    error: Signal<String>,
}

/// Fetch one page of liked tracks and append it to the list.
fn fetch_page(mut ctx: FetchCtx, offset: u32) {
    ctx.loading.set(true);
    ctx.error.set(String::new());
    spawn(async move {
        match api::get_user_saved_tracks(PAGE_SIZE, offset).await {
            Ok(page) => {
                ctx.total.set(page.total);
                ctx.items.write().extend(page.items);
            }
            Err(err) => ctx.error.set(err.to_string()),
        }
        ctx.loading.set(false);
    });
}

#[component]
pub fn Liked() -> Element {
    // Signal handles are cheap Copy values; declare without `mut`.
    let items = use_signal(Vec::<SavedTrack>::new);
    let total = use_signal(|| 0u32);
    let loading = use_signal(|| false);
    let error = use_signal(String::new);
    // First page loads on mount; further pages via the button.
    let mut started = use_signal(|| false);

    let ctx = FetchCtx {
        items,
        total,
        loading,
        error,
    };
    let ctx_for_effect = ctx.clone();
    use_effect(move || {
        if !*started.read() {
            started.set(true);
            fetch_page(ctx_for_effect.clone(), 0);
        }
    });

    // Only entries carrying a playable track render.
    let playable: Vec<_> = items
        .read()
        .iter()
        .filter_map(|s| s.playable().cloned())
        .collect();
    let first_track = playable.first().cloned();
    let shuffle_pool = playable.clone();

    rsx! {
        div { class: "page detail liked",
            HeroHeader {
                kind: "Playlist".to_string(),
                title: "Liked Songs".to_string(),
                meta: format!("{} songs", total()),
                image_url: String::new(),
                seed: "liked-songs".to_string(),
                onplay: move |_| {
                    if let Some(t) = first_track.clone() {
                        crate::player::launch_track(t);
                    }
                },
                onshuffle: move |_| {
                    if !shuffle_pool.is_empty() {
                        let i = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.subsec_nanos() as usize)
                            .unwrap_or(0);
                        crate::player::launch_track(shuffle_pool[i % shuffle_pool.len()].clone());
                    }
                },
            }

            if !error().is_empty() {
                div { class: "error-banner", "{error}" }
            }

            div { class: "track-list",
                for (i, t) in playable.into_iter().enumerate() {
                    TrackRow {
                        key: "{t.id}-{i}",
                        track: t,
                        index: Some((i + 1) as u32),
                        onplay: crate::player::launch_track,
                    }
                }
            }

            div { class: "load-more-row",
                if *loading.read() {
                    div { class: "spinner spinner-sm" }
                } else if (items.read().len() as u32) < *total.read() && total() > 0 {
                    button {
                        class: "ghost",
                        onclick: move |_| fetch_page(ctx.clone(), items.read().len() as u32),
                        "Load more"
                    }
                } else if total() > 0 {
                    span { class: "np-duration", "End of your likes" }
                }
            }
        }
    }
}

// Keep the icon referenced even if the hero owns visuals today.
#[allow(unused)]
fn _icon_keep() -> Element {
    heart(16, false)
}