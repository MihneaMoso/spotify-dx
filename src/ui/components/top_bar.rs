//! Top bar: history navigation, global search hand-off and the account menu.
//!
//! Sits in the shell's `top` grid row on every viewport (slims down on
//! mobile via CSS).

use dioxus::prelude::*;

use crate::state::{AUTH_STATE, SEARCH_SEED};
use crate::ui::icons::{back_arrow, forward_arrow, search as search_icon, settings_gear};
use crate::ui::router::Route;

#[component]
pub fn TopBar() -> Element {
    let nav = navigator();
    let mut query = use_signal(String::new);

    let display_name = AUTH_STATE
        .read()
        .user_display_name
        .clone()
        .unwrap_or_else(|| "You".to_string());
    // First letter of the display name for the avatar dot.
    let initial = display_name
        .chars()
        .next()
        .map(|c| c.to_ascii_uppercase())
        .unwrap_or('·');

    rsx! {
        header { class: "top-bar",
            div { class: "tb-arrows", style: "display:flex;gap:4px;",
                button {
                    class: "tb-btn",
                    title: "Back",
                    onclick: move |_| { nav.go_back(); },
                    {back_arrow(18)}
                }
                button {
                    class: "tb-btn",
                    title: "Forward",
                    onclick: move |_| { nav.go_forward(); },
                    {forward_arrow(18)}
                }
            }

            div { class: "tb-search",
                {search_icon(16)}
                input {
                    r#type: "search",
                    placeholder: "What do you want to play?",
                    value: "{query}",
                    oninput: move |evt| query.set(evt.value()),
                    onkeydown: move |evt| {
                        if evt.key() == Key::Enter {
                            let q = query.read().trim().to_string();
                            SEARCH_SEED.write().replace_range(.., &q);
                            nav.push(Route::Search);
                        }
                    },
                }
            }

            details { class: "avatar-chip",
                summary {
                    span { class: "avatar-dot", "{initial}" }
                    span { title: "{display_name}", "{display_name}" }
                }
                div { class: "avatar-menu",
                    button { class: "menu-item", disabled: true, title: "Settings arrive with the Phase-3 settings page",
                        {settings_gear(16)}
                        "Settings"
                    }
                    div { class: "menu-sep" }
                    button {
                        class: "menu-item",
                        onclick: move |_| crate::auth::logout(),
                        "Log out"
                    }
                }
            }
        }
    }
}