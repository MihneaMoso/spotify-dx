use dioxus::prelude::*;

use crate::state::AUTH_STATE;

/// Sign-in gate shown before the app shell.
#[component]
pub fn Login() -> Element {
    let mut busy = use_signal(|| false);
    let mut started = use_signal(|| false);

    rsx! {
        div { class: "login-page",
            div { class: "login-card",
                div { class: "login-brand",
                    div { class: "brand-mark", "SDX" }
                    h1 { "Spotify DX" }
                    p { "Your library, your feed, no surprises." }
                }
                if busy() {
                    div { class: "login-note",
                        if started() {
                            "Waiting for the browser flow… you can close the dialog once signed in."
                        } else {
                            "Starting the browser…"
                        }
                    }
                    div { class: "spinner" }
                } else {
                    button {
                        class: "primary login-button",
                        onclick: move |_| {
                            busy.set(true);
                            started.set(false);
                            dioxus::prelude::spawn(async move {
                                match crate::auth::login().await {
                                    Ok(auth) => {
                                        AUTH_STATE.write().copy_from(&auth);
                                        crate::player::on_authenticated();
                                    }
                                    Err(err) => {
                                        busy.set(false);
                                        tracing::error!("auth: login failed: {err:#}");
                                        crate::state::publish_error(crate::app_error::AppError::Auth(
                                            format!("Sign-in failed: {err:#}")
                                        ));
                                    }
                                }
                            });
                        },
                        "Continue with Spotify"
                    }
                    p { class: "login-terms",
                        "By continuing you agree to Spotify's Developer Terms.\nTokens stay on this device."
                    }
                }
            }
        }
    }
}