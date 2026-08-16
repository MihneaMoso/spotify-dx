use dioxus::prelude::*;
use futures::channel::mpsc::UnboundedReceiver;

use crate::state::ADBLOCK_STATS;
use crate::ui::components::{BottomNav, PlayerBar, SideNav, Toast};
use crate::ui::router::Route;

/// Sidebar drag-to-resize min/max (px). Tuned to feel snappy without the rail
/// ever becoming unusable narrow or hogging the whole window.
const SIDEBAR_MIN: f64 = 200.0;
const SIDEBAR_MAX: f64 = 460.0;
const SIDEBAR_DEFAULT: f64 = 240.0;

/// Wraps every authenticated page: side nav (wide) / bottom nav (narrow),
/// the outlet for the current route, the persistent player bar and toasts.
#[component]
pub fn AppLayout() -> Element {
    // Sidebar width is state so the drag-to-resize handle can update the grid
    // column in real time via the `--sidebar-width` custom property.
    let mut sidebar_width = use_signal(|| SIDEBAR_DEFAULT);
    // Whether a sidebar resize drag is in flight. Lives here (not in the thin
    // handle) so the whole app-shell can subscribe to pointer moves during it.
    let mut resizing = use_signal(|| false);

    // Keep blocker stats fresh for the nav footer.
    use_coroutine(|_rx: UnboundedReceiver<()>| async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
        loop {
            interval.tick().await;
            let stats = crate::adblock::stats_snapshot();
            ADBLOCK_STATS.write().blocked = stats.blocked;
            ADBLOCK_STATS.write().cached_entries = stats.cached_entries;
            ADBLOCK_STATS.write().ad_fetch_failures = stats.ad_fetch_failures;
        }
    });

    rsx! {
        div {
            class: if resizing() { "app-shell resizing" } else { "app-shell" },
            style: "--sidebar-width: {sidebar_width}px",
            onpointermove: move |evt| {
                if resizing() {
                    let w = evt.client_coordinates().x.clamp(SIDEBAR_MIN, SIDEBAR_MAX);
                    sidebar_width.set(w);
                }
            },
            onpointerup: move |_| resizing.set(false),
            onpointercancel: move |_| resizing.set(false),
            SideNav {
                resizing: resizing,
                onresize: move |w| sidebar_width.set(w),
            }
            div { class: "main-content",
                Outlet::<Route> {}
            }
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