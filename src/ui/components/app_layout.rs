use dioxus::prelude::*;
use futures::channel::mpsc::UnboundedReceiver;

use crate::state::ADBLOCK_STATS;
use crate::ui::components::{BottomNav, PlayerBar, SideNav, Toast};
use crate::ui::router::Route;

/// Wraps every authenticated page: side nav (wide) / bottom nav (narrow),
/// the outlet for the current route, the persistent player bar and toasts.
#[component]
pub fn AppLayout() -> Element {
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
        div { class: "app-shell",
            SideNav {}
            div { class: "main-content",
                Outlet::<Route> {}
            }
            PlayerBar {}
            BottomNav {}
            Toast {}
        }
    }
}