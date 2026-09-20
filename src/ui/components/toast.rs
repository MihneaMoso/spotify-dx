use dioxus::prelude::*;

use crate::state::APP_ERROR;

/// Bottom-toast that surfaces a non-fatal error for ~5 seconds.
#[component]
pub fn Toast() -> Element {
    // Always hook, regardless of whether an error is showing, so the hook
    // count stays stable. Re-runs whenever an error appears (dioxus tracks
    // the signal reads inside the effect).
    use_effect(move || {
        let Some(seen) = APP_ERROR.read().as_ref().map(|stamped| stamped.seq) else {
            return;
        };
        dioxus::prelude::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            if let Some(stamped) = APP_ERROR.peek().as_ref() {
                // Sequence identity: a repeated identical error is a new
                // emission with a new timer — the old timer must not clear
                // it early.
                if stamped.seq == seen {
                    APP_ERROR.write().take();
                }
            }
        });
    });

    let snapshot = APP_ERROR.read();
    let Some(stamped) = snapshot.as_ref() else {
        return VNode::empty();
    };
    let message = stamped.err.to_string();

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
