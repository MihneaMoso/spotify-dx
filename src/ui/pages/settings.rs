//! Settings: appearance (theme), playback engine, privacy, about.
//! The ONLY place in the app that exposes theme/engine switches.

use dioxus::prelude::*;

use crate::profile::{self, PROFILE, PROFILE_ERROR};
use crate::settings::{EnginePreference, ThemeName};
use crate::state::{ADBLOCK_STATS, SETTINGS};
use crate::ui::theme;
use crate::updater::{self, CURRENT_VERSION, UPDATE_READY, UPDATE_STATUS};

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
                h2 { "Profile" }
                p { class: "settings-hint", "Shown in the top bar. The picture is stored locally, on your device." }
                ProfileSection {}
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
                    "Which engine plays audio. Auto uses the Spotify Web Playback SDK for Premium accounts and the open multi-source engine (TIDAL/Qobuz/YouTube) for free accounts — every user gets full-track playback."
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
                h2 { "Cosmetic filtering" }
                p { class: "settings-hint",
                    "Hide premium upsell UI (upgrade buttons, HPTO banners, sponsored items) in the login WebView. Disable if Spotify pages look broken."
                }
                UpsellToggle {}
            }

            section { class: "settings-section",
                h2 { "Software & updates" }
                p { class: "settings-hint",
                    "Releases ship as desktop tarballs and an Android APK on GitHub.
                    Checking here downloads and stages the newest build; applying restarts the app."
                }
                div { class: "update-row",
                    span { class: "update-version", "Spotify DX v{CURRENT_VERSION}" }
                    button {
                        class: "menu-item radio-row",
                        onclick: move |_| updater::run_check(),
                        "Check for updates"
                    }
                }
                if *UPDATE_READY.read() {
                    button {
                        class: "menu-item radio-row primary",
                        onclick: move |_| { updater::apply_now(); },
                        "Apply update now"
                    }
                }
                UpdateToggle {}
                if let Some(status) = UPDATE_STATUS.read().clone() {
                    p { class: "update-status", "{status}" }
                }
            }

            section { class: "settings-section",
                h2 { "Cache" }
                p { class: "settings-hint",
                    "Artwork & API snapshots are cached to disk automatically."
                }
            }
        }
    }
}

/// Check-for-updates-at-startup toggle. Persisted in settings.json.
#[component]
fn UpdateToggle() -> Element {
    let on = SETTINGS.read().auto_check_updates;

    rsx! {
        button {
            class: if on { "menu-item radio-row active" } else { "menu-item radio-row" },
            onclick: move |_| {
                SETTINGS.write().auto_check_updates = !on;
                let snapshot = *SETTINGS.peek();
                dioxus::prelude::spawn(async move {
                    let _ = snapshot.save();
                });
            },
            span { class: if on { "radio-dot on" } else { "radio-dot" } }
            if on { "Check for updates at startup: ON" } else { "Check for updates at startup: OFF" }
        }
    }
}

/// Profile name + avatar editor, backed directly by the global [`PROFILE`]
/// signal (saved on commit).
#[component]
fn ProfileSection() -> Element {
    let username = PROFILE.read().username.clone();
    let error = PROFILE_ERROR.read().clone();

    rsx! {
        div { class: "profile-edit",
            AvatarPreview { size: 64 }
            div { class: "profile-edit-fields",
                label { class: "field-label", "User name" }
                input {
                    r#type: "text",
                    placeholder: "Your name",
                    value: "{username}",
                    oninput: move |evt| PROFILE.write().username = evt.value(),
                    onchange: move |_| profile::save(),
                }
                div { class: "profile-actions",
                    label { class: "file-upload",
                        span { "Upload picture" }
                        input {
                            r#type: "file",
                            accept: "image/*",
                            onchange: move |e: Event<FormData>| {
                                let Some(file) = e.files().first().cloned() else { return; };
                                let mime = file.content_type();
                                spawn(async move {
                                    match file.read_bytes().await {
                                        Ok(bytes) => {
                                            PROFILE_ERROR.write().clear();
                                            let result = profile::set_avatar(
                                                &mut PROFILE.write(),
                                                mime,
                                                &bytes,
                                            );
                                            match result {
                                                Ok(()) => profile::save(),
                                                Err(msg) => *PROFILE_ERROR.write() = msg,
                                            }
                                        }
                                        Err(err) => {
                                            *PROFILE_ERROR.write() =
                                                format!("Could not read image: {err}");
                                        }
                                    }
                                });
                            },
                        }
                    }
                    if PROFILE.read().has_avatar() {
                        button {
                            r#type: "button",
                            onclick: move |_| {
                                profile::clear_avatar(&mut PROFILE.write());
                                profile::save();
                            },
                            "Remove"
                        }
                    }
                }
            }
        }
        if !error.is_empty() {
            p { class: "settings-error", "{error}" }
        }
    }
}

/// Circular avatar preview (image, or initials fallback) reading the global
/// profile, so the top bar and settings stay in sync.
#[component]
fn AvatarPreview(size: u32) -> Element {
    let uri = PROFILE.read().avatar_data_uri();
    let initials = PROFILE.read().initials();

    if let Some(uri) = uri {
        rsx! {
            img {
                class: "profile-avatar",
                src: uri,
                "aria-label": "Profile picture",
                alt: "",
                width: "{size}",
                height: "{size}",
            }
        }
    } else {
        rsx! {
            div { class: "profile-avatar profile-avatar-initials", style: "width:{size}px;height:{size}px;font-size:{size / 3}px",
                "{initials}"
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

#[component]
fn UpsellToggle() -> Element {
    let on = SETTINGS.read().hide_upsell;

    rsx! {
        button {
            class: if on { "menu-item radio-row active" } else { "menu-item radio-row" },
            onclick: move |_| {
                SETTINGS.write().hide_upsell = !on;
                let snapshot = *SETTINGS.peek();
                dioxus::prelude::spawn(async move {
                    let _ = snapshot.save();
                });
            },
            span { class: if on { "radio-dot on" } else { "radio-dot" } }
            if on { "Hide upsell: ON" } else { "Hide upsell: OFF" }
        }
    }
}