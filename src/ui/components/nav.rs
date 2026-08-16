use dioxus::prelude::*;

use crate::state::ADBLOCK_STATS;
use crate::ui::components::SidebarResizer;
use crate::ui::icons::{home, library, search};
use crate::ui::router::Route;

/// Desktop/wide layout left rail. Not shown on narrow/mobile viewports.
#[component]
pub fn SideNav(resizing: Signal<bool>, onresize: EventHandler<f64>) -> Element {
    let blocked = ADBLOCK_STATS.read().blocked;
    let cached = ADBLOCK_STATS.read().cached_entries;

    rsx! {
        nav { class: "side-nav",
            div { class: "brand",
                div { class: "brand-mark", "SDX" }
                div { class: "brand-name", "Spotify DX" }
            }
            div { class: "nav-section",
                Link { to: Route::Home, class: "nav-item", {home(22)}, span { "Home" } }
                Link { to: Route::Search, class: "nav-item", {search(22)}, span { "Search" } }
                Link { to: Route::Library, class: "nav-item", {library(22)}, span { "Your Library" } }
            }
            div { class: "nav-footer",
                div { class: "adblock-badge", title: "Blocklist entries loaded on disk",
                    "{cached} rules cached" }
                div { class: "adblock-badge blocked", title: "Requests blocked by the ad filter",
                    "{blocked} blocked" }
                details { class: "nav-note",
                    summary { "About ad-block" }
                    p { style: "font-size:12px;max-width:32ch;line-height:1.5;color:var(--muted-2);",
                        "Media requests pass through a local blocklist that mirrors Spotify's ad hosts. This applies to playback gate/ad-pipelined requests and the 'premium preview' wall, not to your account." }
                }
                a { href: "https://adguardteam.github.io/AdGuardSDNSFilter/Filters/filter.txt", target: "_blank",
                    rel: "external noopener",
                    class: "nav-link",
                    "AdGuard DNS filter →"
                }
            }
            if crate::state::is_blocker_ready() {
                div { class: "nav-ready", "Blocker active" }
            }
            SidebarResizer { resizing, onresize }
        }
    }
}

/// Mobile bottom navigation. Hidden on wide viewports (alternates with SideNav).
#[component]
pub fn BottomNav() -> Element {
    rsx! {
        nav { class: "bottom-nav",
            Link { to: Route::Home, class: "bottom-item", {home(22)}, span { "Home" } }
            Link { to: Route::Search, class: "bottom-item", {search(22)}, span { "Search" } }
            Link { to: Route::Library, class: "bottom-item", {library(22)}, span { "Library" } }
        }
    }
}