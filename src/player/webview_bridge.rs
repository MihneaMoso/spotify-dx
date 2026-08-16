use crate::player::playback_sdk;
use crate::player::playback_sdk::SdkState;
use crate::state::{AUTH_STATE, PLAYER_STATE};
use anyhow::Context as _;
use dioxus::signals::Readable;
use std::cell::RefCell;
use std::sync::OnceLock;

use wry::{Rect, WebView, WebViewBuilder};

// The single hidden WebView that runs the Web Playback SDK for the whole app
// lifetime, owned by the UI thread. dioxus-desktop polls the whole virtual DOM
// on the main thread, and webkitgtk requires the WebView to be touched only
// there, so a `thread_local` is both correct and `Send`-free (hence compatible
// with the crate-level `#![forbid(unsafe_code)]`).
thread_local! {
    static WEBVIEW: RefCell<Option<WebView>> = const { RefCell::new(None) };
}

// JS → Rust traffic is dropped into this queue by the wry IPC handler and
// drained by a dioxus task (which runs inside a `Runtime` context). Touching
// `Global` signals directly from the IPC handler panics, because that callback
// runs on the webkit thread with no active dioxus runtime.
static IPC_QUEUE: OnceLock<tokio::sync::mpsc::UnboundedSender<serde_json::Value>> =
    OnceLock::new();

/// Create (once) the off-screen WebView hosting `SDK_HTML` and wire the JS↔Rust
/// message channel. Must run on the UI thread after dioxus has built a window.
pub fn init() -> anyhow::Result<()> {
    if WEBVIEW.with(|cell| cell.borrow().is_some()) {
        return Ok(());
    }

    let desktop = dioxus::desktop::window();

    #[cfg(target_os = "linux")]
    let builder = {
        use dioxus::desktop::tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix as _;
        let vbox = desktop
            .window
            .default_vbox()
            .context("had no gtk container to host the hidden webview")?;
        WebViewBuilder::new_gtk(vbox)
    };

    #[cfg(not(target_os = "linux"))]
    let mut builder = WebViewBuilder::new(&desktop.window);

    let webview = builder
        .with_html(playback_sdk::SDK_HTML)
        // Never visible: 1×1, pushed far off-screen.
        .with_bounds(Rect {
            position: wry::dpi::Position::Physical(wry::dpi::PhysicalPosition::new(0, -9999)),
            size: wry::dpi::Size::Logical(wry::dpi::LogicalSize::new(1.0, 1.0)),
        })
        .with_visible(false)
        .with_ipc_handler(handle_ipc)
        .build()
        .context("failed to build the hidden webview")?;

    WEBVIEW.with(move |cell| *cell.borrow_mut() = Some(webview));

    // Spawn the message dispatcher so IPC is handled inside a dioxus runtime.
    if IPC_QUEUE.get().is_none() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<serde_json::Value>();
        let _ = IPC_QUEUE.set(tx);
        dioxus::prelude::spawn(async move {
            while let Some(msg) = rx.recv().await {
                handle_message(&msg);
            }
        });
    }
    Ok(())
}

/// JS → Rust: parse `window.ipc.postMessage` payloads. Runs on the webkit
/// thread, so it only enqueues; the drain task does the real work.
fn handle_ipc(request: wry::http::Request<String>) {
    let body = request.into_body();
    let Ok(msg) = serde_json::from_str::<serde_json::Value>(&body) else {
        tracing::debug!("webview: ignored malformed ipc payload");
        return;
    };
    if let Some(tx) = IPC_QUEUE.get() {
        let _ = tx.send(msg);
    }
}

/// Process a queued IPC message inside the dioxus runtime.
fn handle_message(msg: &serde_json::Value) {
    let body = msg.to_string();
    let kind = msg.get("type").and_then(|t| t.as_str()).unwrap_or_default();
    match kind {
        // The SDK asks for a token: echo the current one back.
        "needToken" => {
            let token = AUTH_STATE.peek().access_token.clone().unwrap_or_default();
            provide_token(&token);
        }
        "ready" => {
            let device_id = msg.get("device_id").and_then(|d| d.as_str()).unwrap_or_default();
            tracing::info!("webview: sdk ready on device {device_id}");
            PLAYER_STATE.write().device_id = (!device_id.is_empty()).then(|| device_id.to_owned());
        }
        "not_ready" => {
            tracing::info!("webview: sdk released the device");
            PLAYER_STATE.write().device_id = None;
        }
        "state" => {
            if let Some(payload) = msg.get("payload") {
                apply_state(payload);
            }
        }
        "auth_error" | "init_error" => {
            let message = msg.get("message").and_then(|m| m.as_str()).unwrap_or_default();
            tracing::warn!("webview: sdk error ({kind}): {message}");
        }
        _ => tracing::trace!("webview: unhandled ipc message {body}"),
    }
}

/// Apply a player-state-changed payload to `PLAYER_STATE`.
fn apply_state(payload: &serde_json::Value) {
    let state: SdkState = playback_sdk::parse_sdk_state(payload);
    PLAYER_STATE.write().track = state.track;
    PLAYER_STATE.write().is_playing = state.is_playing;
    PLAYER_STATE.write().position_ms = state.position_ms;
    PLAYER_STATE.write().duration_ms = state.duration_ms;
    PLAYER_STATE.write().shuffle = state.shuffle;
    PLAYER_STATE.write().repeat = match state.repeat {
        1 => crate::state::RepeatMode::Context,
        2 => crate::state::RepeatMode::Track,
        _ => crate::state::RepeatMode::Off,
    };
}

fn eval(js: &str) {
    WEBVIEW.with(|cell| {
        if let Some(webview) = cell.borrow().as_ref() {
            if let Err(err) = webview.evaluate_script(js) {
                tracing::debug!("webview: eval failed: {err}");
            }
        }
    });
}

/// Rust → JS: hand the OAuth token to the SDK.
pub fn provide_token(token: &str) {
    let js = format!("window._relay.provideToken({:?})", token);
    eval(&js);
}

/// (Re)connect the SDK after a fresh login. The hidden WebView boots before the
/// user authenticates, so the initial `connect()` dies with an auth error; once
/// we have a token we must hand it over and ask the player to connect again.
pub fn reconnect() {
    let token = AUTH_STATE.peek().access_token.clone().unwrap_or_default();
    if token.is_empty() {
        return;
    }
    let js = format!("window._relay.provideToken({token:?}); window._relay.connect();");
    eval(&js);
}

pub fn play() {
    eval("window._relay.play()");
}

pub fn pause() {
    eval("window._relay.pause()");
}

pub fn next() {
    eval("window._relay.next()");
}

pub fn prev() {
    eval("window._relay.prev()");
}

pub fn seek(ms: u64) {
    eval(&format!("window._relay.seek({ms})"));
}

pub fn volume(v: f32) {
    eval(&format!("window._relay.volume({v})"));
}