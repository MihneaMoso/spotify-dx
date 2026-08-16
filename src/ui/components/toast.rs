use dioxus::prelude::*;

use crate::state::APP_ERROR;

/// Bottom-toast that surfaces a non-fatal error for ~5 seconds.
#[component]
pub fn Toast() -> Element {
    // Always hook, regardless of whether an error is showing, so the hook
    // count stays stable. Re-runs whenever an error appears (dioxus tracks
    // the signal reads inside the effect).
    use_effect(move || {
        let Some(seen) = APP_ERROR.read().as_ref().map(|err| err.to_string()) else {
            return;
        };
        dioxus::prelude::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            if let Some(err) = APP_ERROR.peek().as_ref() {
                if err.to_string() == seen {
                    APP_ERROR.write().take();
                }
            }
        });
    });

    let snapshot = APP_ERROR.read();
    let Some(message) = snapshot.as_ref() else {
        return VNode::empty();
    };
    let message = message.to_string();

    rsx! {
        div { class: "toast",
            div { class: "toast-body", "{message}" }
            button {
                class: "toast-dismiss",
                onclick: move |_| { APP_ERROR.write().take(); },
                "Dismiss"
            }
        }
    }
}