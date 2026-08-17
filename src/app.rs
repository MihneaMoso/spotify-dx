use dioxus::document::Link;
use dioxus::prelude::*;

use crate::state::AUTH_STATE;
use crate::ui::pages::Login;
use crate::ui::router::Route;

/// Root component: decides between the login gate and the routed app shell.
///
/// The login gate shows the real `open.spotify.com` sign-in (the web-session
/// WebView) even when a keychain token exists — that page is the session's
/// source of truth, and tokens are refreshed through it.
#[component]
pub fn App() -> Element {
    // Whether the playback backend was already booted and whether the profile
    // backfill was already spawned. `AUTH_STATE` is written on every token
    // refresh (~every 1.5s), which re-renders this component and re-runs the
    // effect — the gates keep the one-time boots from repeating (re-spawning a
    // profile fetch that keeps failing would feed the api.spotify.com rate
    // limit that blocked it, and re-evaluating `connect()` in the SDK is
    // pointless churn).
    let mut backend_booted = use_signal(|| false);
    let mut profile_backfilled = use_signal(|| false);

    // Once we have a session, boot the playback backend. This runs on the UI
    // thread where the dioxus window exists (the hidden SDK WebView must be
    // created there), so it is deliberately not done in main.rs.
    use_effect(move || {
        if AUTH_STATE.read().is_authenticated {
            if !*backend_booted.read() {
                backend_booted.set(true);
                let _ = crate::player::init();
                crate::player::on_authenticated();
            }
            // Fast-path restores skip the profile fetch when the bootstrap
            // runtime was dropped mid-flight — backfill it here, once.
            if AUTH_STATE.peek().user_id.is_none() && !*profile_backfilled.read() {
                profile_backfilled.set(true);
                dioxus::prelude::spawn(crate::auth::refresh_profile());
            }
        }
    });

    let authenticated = AUTH_STATE.read().is_authenticated;

    rsx! {
        // Load the design system once. The head component is deduplicated by
        // href, so re-renders of the login gate / shell never re-inject it.
        Link { rel: "stylesheet", href: asset!("/assets/main.css") }
        if authenticated {
            Router::<Route> {}
        } else {
            Login {}
        }
    }
}
