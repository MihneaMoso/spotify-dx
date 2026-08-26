use crate::app_error::AppError;
use crate::player::playback_sdk;
use crate::player::playback_sdk::SdkState;
use crate::state::{AUTH_STATE, PLAYER_STATE};
use anyhow::Context as _;
use std::cell::RefCell;
use std::sync::{Mutex, OnceLock};

use wry::{Rect, WebView, WebViewBuilder};

// The hidden SDK WebView. It is built with the process-wide session WebContext
// (see `crate::auth::session_context_mut`), which is `Send`-free (webkitgtk
// must be touched only on the UI thread), so it lives in a `thread_local` —
// compatible with `#![forbid(unsafe_code)]`.
struct SdkWebView {
    webview: WebView,
}

thread_local! {
    static WEBVIEW: RefCell<Option<SdkWebView>> = const { RefCell::new(None) };
}

// JS → Rust traffic is dropped into this queue by the wry IPC handler and
// drained by a dioxus task (which runs inside a `Runtime` context). Touching
// `Global` signals directly from the IPC handler panics, because that callback
// runs on the webkit thread with no active dioxus runtime.
static IPC_QUEUE: OnceLock<tokio::sync::mpsc::UnboundedSender<serde_json::Value>> =
    OnceLock::new();

/// Pending token-refresh requests. `request_token_refresh()` pushes its sender
/// here; the IPC handler answers EVERY pending sender when the JS fetch
/// round-trips. Multiple requests can legitimately be outstanding at once (Home
/// and the profile backfill both refresh after login) — a single overwritten
/// slot would silently drop the first sender, which surfaces as a spurious
/// "Token refresh timed out".
static REFRESH_TX: Mutex<Vec<tokio::sync::oneshot::Sender<Result<String, String>>>> =
    Mutex::new(Vec::new());

/// Create (once) the off-screen WebView hosting `SDK_HTML` and wire the JS↔Rust
/// message channel. Must run on the UI thread after dioxus has built a window.
///
/// The WebView shares the login WebView's data directory, so it inherits the
/// user's Spotify session cookies and can fetch its own access tokens — Rust
/// never has to hand it a token.
pub fn init() -> anyhow::Result<()> {
    if WEBVIEW.with(|cell| cell.borrow().is_some()) {
        return Ok(());
    }

    let desktop = dioxus::desktop::window();

    // Same shared cookie jar as the sign-in WebView: the SDK behaves as the
    // authenticated user with zero token wiring. ONE process-wide context is
    // used by every WebView — a second WebContext on the same data directory
    // makes webkitgtk abort. Build inside the closure because the context
    // borrow cannot escape the `thread_local`'s `.with`.
    let webview = crate::auth::with_session_context(|context| {
        let builder = WebViewBuilder::new_with_web_context(context)
            .with_html(playback_sdk::SDK_HTML)
            // Never visible: 1×1, pushed far off-screen.
            .with_bounds(Rect {
                position: wry::dpi::Position::Physical(wry::dpi::PhysicalPosition::new(0, -9999)),
                size: wry::dpi::Size::Logical(wry::dpi::LogicalSize::new(1.0, 1.0)),
            })
            .with_visible(false)
            .with_ipc_handler(handle_ipc);

        #[cfg(target_os = "linux")]
        {
            use dioxus::desktop::tao::platform::unix::WindowExtUnix;
            use wry::WebViewBuilderExtUnix as _;
            let vbox = desktop
                .window
                .default_vbox()
                .context("had no gtk container to host the hidden webview")?;
            builder
                .build_gtk(vbox)
                .context("failed to build the hidden webview")
        }

        #[cfg(not(target_os = "linux"))]
        {
            builder
                .build(&desktop.window)
                .context("failed to build the hidden webview")
        }
    })?;

    WEBVIEW.with(move |cell| {
        *cell.borrow_mut() = Some(SdkWebView { webview })
    });

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

/// Destroy the hidden WebView and its cookie-holding process. Used by logout so
/// a fresh WebView (and fresh login) starts from a clean state.
pub fn shutdown() {
    WEBVIEW.with(|cell| *cell.borrow_mut() = None);
}

/// JS → Rust: parse `window.ipc.postMessage` payloads. Runs on the webkit
/// thread, so it only enqueues; the drain task does the real work. Also used by
/// the in-window session WebView (`auth::webview_login`), which shares the IPC
/// channel so its token-refresh answers land here too.
pub(crate) fn handle_ipc(request: wry::http::Request<String>) {
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
        // The SDK's getOAuthToken fetched a token itself — keep AUTH_STATE current.
        "token_refresh" => apply_token_msg(msg, false),
        // Rust requested a refresh via request_token_refresh() — answer it.
        "token_refresh_result" => apply_token_msg(msg, true),
        "token_error" => {
            let message = msg.get("msg").and_then(|m| m.as_str()).unwrap_or_default();
            tracing::error!("webview: token fetch error: {message}");
        }
        "token_debug" => {
            let message = msg.get("msg").and_then(|m| m.as_str()).unwrap_or_default();
            tracing::info!("webview: token debug: {message}");
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

/// Fold a `token_refresh[_result]` payload into `AUTH_STATE` (and keychain),
/// and optionally answer the pending `REFRESH_TX` oneshot.
fn apply_token_msg(msg: &serde_json::Value, answer_refresh: bool) {
    let token = msg.get("token").and_then(|t| t.as_str()).unwrap_or("").to_string();
    let expires_ms = msg.get("expiresMs").and_then(|t| t.as_u64()).unwrap_or(0);
    let is_anon = msg.get("isAnon").and_then(|t| t.as_bool()).unwrap_or(true);

    if is_anon {
        tracing::warn!("webview: session expired");
        crate::state::publish_error(AppError::SessionExpired);
        AUTH_STATE.write().is_authenticated = false;
        // Tear the session WebView down so a fresh login can start (start()
        // refuses to run while one is alive).
        crate::auth::webview_login::shutdown();
        if answer_refresh {
            for tx in REFRESH_TX.lock().unwrap().drain(..) {
                let _ = tx.send(Err("session expired".into()));
            }
        }
        return;
    }

    crate::auth::token_store::save(&token, expires_ms);
    {
        let mut s = AUTH_STATE.write();
        s.access_token = Some(token.clone());
        // A capture carrying no expiry field (or one the JS couldn't read)
        // arrives as `expiresMs: 0`. Never let that clobber a valid stored
        // expiry — a poisoned 0 would force every page into the refresh path.
        if expires_ms != 0 {
            s.expires_at_ms = expires_ms;
        }
    }
    if answer_refresh {
        for tx in REFRESH_TX.lock().unwrap().drain(..) {
            let _ = tx.send(Ok(token.clone()));
        }
    }
}


/// Apply a player-state-changed payload to `PLAYER_STATE`.
fn apply_state(payload: &serde_json::Value) {
    let state: SdkState = playback_sdk::parse_sdk_state(payload);
    let mut s = PLAYER_STATE.write();
    s.track = state.track;
    s.is_playing = state.is_playing;
    s.position_ms = state.position_ms;
    s.duration_ms = state.duration_ms;
    s.shuffle = state.shuffle;
    s.repeat = match state.repeat {
        1 => crate::state::RepeatMode::Context,
        2 => crate::state::RepeatMode::Track,
        _ => crate::state::RepeatMode::Off,
    };
}

fn eval(js: &str) {
    WEBVIEW.with(|cell| {
        if let Some(sdk) = cell.borrow().as_ref() {
            if let Err(err) = sdk.webview.evaluate_script(js) {
                tracing::debug!("webview: eval failed: {err}");
            }
        }
    });
}

/// Rust → JS: ask for a fresh access token. The in-window session WebView is
/// preferred: its page IS `open.spotify.com`, so its `get_access_token` fetch
/// is same-origin and not CORS-blocked (the SDK WebView's null-origin page
/// fails with `TypeError: Load failed`). Falls back to the SDK WebView when no
/// session WebView is alive. The receiver resolves when the IPC answer
/// arrives, or errors immediately if no WebView is available.
pub fn request_token_refresh() -> tokio::sync::oneshot::Receiver<Result<String, String>> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut guard = REFRESH_TX.lock().unwrap();
    if crate::auth::webview_login::refresh_token() {
        guard.push(tx);
        return rx;
    }
    let has_webview = WEBVIEW.with(|cell| cell.borrow().is_some());
    if !has_webview {
        // No WebView alive (session torn down): fail fast so callers can route
        // the user back to the login page instead of hanging.
        for tx in guard.drain(..) {
            let _ = tx.send(Err("no webview available".into()));
        }
        return rx;
    }
    guard.push(tx);
    drop(guard);
    eval("window._relay && window._relay.refreshToken && window._relay.refreshToken()");
    rx
}

/// (Re)connect the SDK after a fresh login. The SDK fetches its own token via
/// the shared session cookies, so no token hand-over is needed.
pub fn reconnect() {
    eval("window._relay && window._relay.connect && window._relay.connect()");
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

