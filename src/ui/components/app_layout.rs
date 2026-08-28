use dioxus::prelude::*;
use futures::channel::mpsc::UnboundedReceiver;

use crate::state::{ADBLOCK_STATS, SHOW_NOW_PLAYING};
use crate::ui::components::{BottomNav, NowPlayingView, PlayerBar, SideNav, Toast, TopBar};
use crate::ui::router::Route;

/// Sidebar drag-to-resize min/max (px). Tuned to feel snappy without the rail
/// ever becoming unusable narrow or hogging the whole window.
const SIDEBAR_MIN: f64 = 200.0;
const SIDEBAR_MAX: f64 = 460.0;
const SIDEBAR_DEFAULT: f64 = 240.0;

/// Width of the now-playing column when it is shown. CSS drops the column out
/// of the grid entirely below 1280 px regardless of this value.
const NOW_PLAYING_WIDTH: f64 = 320.0;

/// Wraps every authenticated page: top bar, side nav (wide) / icon rail
/// (≤999 px) / bottom nav (narrow), the outlet for the current route, the
/// optional now-playing column, the persistent player bar and toasts.
#[component]
pub fn AppLayout() -> Element {
    // Sidebar width is state so the drag-to-resize handle can update the grid
    // column in real time via the `--sidebar-width` custom property.
    let mut sidebar_width = use_signal(|| SIDEBAR_DEFAULT);
    // Whether a sidebar resize drag is in flight. Lives here (not in the thin
    // handle) so the whole app-shell can subscribe to pointer moves during it.
    let mut resizing = use_signal(|| false);

    // Mirror blocker stats into the nav footer. Event-driven: we await the
    // adblock module's `STATS_CHANGED` notify (fired on drop / blocklist
    // refresh) rather than polling on a 1s timer, so this task sleeps forever
    // while idle. Compare-before-write keeps no-op state changes from
    // re-rendering every subscribed component.
    use_coroutine(|_rx: UnboundedReceiver<()>| async move {
        loop {
            crate::adblock::stats_changed().await;
            // Drain coalesced notifies by re-reading until the snapshot stops
            // changing; a burst of drops lands as at most one render.
            loop {
                let stats = crate::adblock::stats_snapshot();
                let prev = *ADBLOCK_STATS.peek();
                if prev != stats {
                    ADBLOCK_STATS.write().blocked = stats.blocked;
                    ADBLOCK_STATS.write().cached_entries = stats.cached_entries;
                    ADBLOCK_STATS.write().ad_fetch_failures = stats.ad_fetch_failures;
                    // Snapshot may have changed again while we were writing.
                    if crate::adblock::stats_snapshot() == *ADBLOCK_STATS.peek() {
                        break;
                    }
                } else {
                    break;
                }
            }
        }
    });

    let np_width = if *SHOW_NOW_PLAYING.read() {
        NOW_PLAYING_WIDTH
    } else {
        0.0
    };

    rsx! {
        div {
            class: if resizing() { "app-shell resizing" } else { "app-shell" },
            style: "--sidebar-width: {sidebar_width}px; --np-width: {np_width}px",
            onpointermove: move |evt| {
                if resizing() {
                    let w = evt.client_coordinates().x.clamp(SIDEBAR_MIN, SIDEBAR_MAX);
                    sidebar_width.set(w);
                }
            },
            onpointerup: move |_| resizing.set(false),
            onpointercancel: move |_| resizing.set(false),
            TopBar {}
            SideNav {
                resizing: resizing,
                onresize: move |w| sidebar_width.set(w),
            }
            div { class: "main-content",
                Outlet::<Route> {}
            }
            NowPlayingView {}
            PlayerBar {}
            BottomNav {}
            Toast {}
        }
    }
}

/// Thin vertical grab-handle on the rail's right edge. Only starts the drag —
/// the actual move/up handling happens on the app shell so the pointer can
/// leave the handle without losing the interaction. The pointer is also
/// captured via JS so the drag continues even outside the window.
#[component]
pub fn SidebarResizer(
    resizing: Signal<bool>,
    onresize: EventHandler<f64>,
) -> Element {
    rsx! {
        div {
            id: "sidebar-resizer",
            class: if resizing() { "sidebar-resizer active" } else { "sidebar-resizer" },
            aria_label: "Resize sidebar",
            onpointerdown: move |evt| {
                evt.prevent_default();
                evt.stop_propagation();
                // Capture the pointer so this handle keeps receiving the events
                // even when the cursor moves off it / out of the window.
                let pid = evt.pointer_id();
                _ = dioxus::document::eval(&format!(
                    "(function(){{ const el = document.getElementById('sidebar-resizer'); if (el && el.setPointerCapture) el.setPointerCapture({pid}); }})()"
                ));
                resizing.set(true);
            },
            onclick: move |evt| evt.stop_propagation(),
        }
    }
}