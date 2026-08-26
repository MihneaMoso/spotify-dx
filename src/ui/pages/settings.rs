//! Settings: appearance (theme), playback engine, privacy, about.
//! The ONLY place in the app that exposes theme/engine switches.

use dioxus::prelude::*;

use crate::settings::{EnginePreference, ThemeName};
use crate::state::{ADBLOCK_STATS, SETTINGS};
use crate::ui::theme;

#[component]
pub fn Settings() -> Element {
    let current_theme = SETTINGS.read().theme;
    let engine = SETTINGS.read().engine;
    let stats = ADBLOCK_STATS.read();

    let set_engine = move |e: EnginePreference| {
        SETTINGS.write().engine = e;
        let snapshot = *SETTINGS.peek();
        dioxus::prelude::spawn(async move {
            let _ = snapshot.save();
        });
    };

    rsx! {
        div { class: "page settings",
            header { class: "page-header",
                h1 { "Settings" }
                span { class: "subhead", "Preferences persist to settings.json" }
            }

            section { class: "settings-section",
                h2 { "Appearance" }
                p { class: "settings-hint", "Theme applies instantly and persists." }
                div { class: "theme-row",
                    ThemeCard {
                        name: ThemeName::DeepBlue,
                        active: current_theme == ThemeName::DeepBlue,
                    }
                    ThemeCard {
                        name: ThemeName::Onyx,
                        active: current_theme == ThemeName::Onyx,
                    }
                }
            }

            section { class: "settings-section",
                h2 { "Playback" }
                p { class: "settings-hint",
                    "Which engine plays audio. Auto uses the Spotify Web Playback SDK when your account allows it, the open multi-source engine otherwise."
                }
                div { class: "radio-col",
                    EngineRadio {
                        label: "Auto (recommended)",
                        pref: EnginePreference::Auto,
                        active: engine == EnginePreference::Auto,
                        onset: set_engine,
                    }
                    EngineRadio {
                        label: "Spotify Web Playback SDK (Premium)",
                        pref: EnginePreference::SpotifySdk,
                        active: engine == EnginePreference::SpotifySdk,
                        onset: set_engine,
                    }
                    EngineRadio {
                        label: "Open engine (multi-source)",
                        pref: EnginePreference::Open,
                        active: engine == EnginePreference::Open,
                        onset: set_engine,
                    }
                }
            }

            section { class: "settings-section",
                h2 { "Privacy" }
                div { class: "stat-grid",
                    div { class: "stat-card",
                        span { class: "stat-value", "{stats.blocked}" }
                        span { class: "stat-label", "requests blocked" }
                    }
                    div { class: "stat-card",
                        span { class: "stat-value", "{stats.cached_entries}" }
                        span { class: "stat-label", "blocklist rules" }
                    }
                    div { class: "stat-card",
                        span { class: "stat-value", "{stats.ad_fetch_failures}" }
                        span { class: "stat-label", "list refresh failures" }
                    }
                }
                a {
                    class: "nav-link",
                    href: "https://adguardteam.github.io/AdGuardSDNSFilter/Filters/filter.txt",
                    target: "_blank",
                    rel: "external noopener",
                    "Blocklist source (AdGuard DNS filter) →"
                }
            }

            section { class: "settings-section",
                h2 { "Cache" }
                p { class: "settings-hint",
                    "Artwork & API snapshot management arrives with the Phase-4 cache work."
                }
            }
        }
    }
}

/// Clickable theme preview card.
#[component]
fn ThemeCard(name: ThemeName, active: bool) -> Element {
    rsx! {
        button {
            class: if active { "theme-card active" } else { "theme-card" },
            onclick: move |_| theme::set_theme(name),
            div { class: "theme-swatch theme-swatch-{name.attr_value()}",
                span { class: "swatch-accent" }
                span { class: "swatch-surface" }
                span { class: "swatch-bg" }
            }
            span { class: "theme-name", "{name.attr_value()}" }
            if active {
                span { class: "theme-active", "Active" }
            }
        }
    }
}

#[component]
fn EngineRadio(
    label: String,
    pref: EnginePreference,
    active: bool,
    mut onset: EventHandler<EnginePreference>,
) -> Element {
    rsx! {
        button {
            class: if active { "menu-item radio-row active" } else { "menu-item radio-row" },
            onclick: move |_| onset(pref),
            span { class: if active { "radio-dot on" } else { "radio-dot" } }
            "{label}"
        }
    }
}