use crate::state::AUTH_STATE;
use dioxus::prelude::*;

/// The login gate shown when no valid session exists at startup (or after the
/// session expired). It opens the real `open.spotify.com` sign-in in a WebView
/// window; the moment `AUTH_STATE.is_authenticated` flips, `App` swaps to the
/// router shell and this component unmounts.
#[component]
pub fn Login() -> Element {
    let mut error = use_signal(String::new);
    let mut started = use_signal(|| false);

    // Kick the login flow once. `started` gates re-entry so a slow window never
    // gets opened twice; retry resets it.
    use_effect(move || {
        if AUTH_STATE.read().is_authenticated || *started.read() {
            return;
        }
        started.set(true);
        dioxus::prelude::spawn(async move {
            if let Err(err) = crate::auth::login().await {
                error.set(err.to_string());
                started.set(false);
            }
        });
    });

    rsx! {
        div { class: "login-overlay",
            div { class: "login-brand",
                span { class: "login-logo", "♫" }
                h1 { "Spotify DX" }
                if error.read().is_empty() {
                    p { class: "login-hint", "Opening Spotify login…" }
                } else {
                    p { class: "login-error", "Couldn't start Spotify login: {error}" }
                    button {
                        class: "login-retry",
                        onclick: move |_| error.set(String::new()),
                        "Try again"
                    }
                }
            }
        }
    }
}