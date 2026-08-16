use dioxus::prelude::*;

use crate::state::AUTH_STATE;
use crate::ui::pages::Login;
use crate::ui::router::Route;

/// Root component: decides between the login gate and the routed app shell.
#[component]
pub fn App() -> Element {
    // Seed the auth signal from the pre-launch boot snapshot once the DOM
    // exists, so we don't flash the login screen on a persisted session.
    use_effect(move || {
        if let Some(auth) = crate::auth::take_boot_auth() {
            AUTH_STATE.write().copy_from(&auth);
        }
        let _ = crate::player::init();
        crate::player::on_authenticated();
    });

    let authenticated = AUTH_STATE.read().is_authenticated();

    if authenticated {
        rsx! {
            Router::<Route> {}
        }
    } else {
        rsx! {
            Login {}
        }
    }
}