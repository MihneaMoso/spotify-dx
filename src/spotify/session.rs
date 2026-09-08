use crate::app_error::AppError;
use crate::spotify::Result;
use crate::state::AUTH_STATE;
use chrono::Utc;
use dioxus::prelude::ReadableExt;

/// Headless-core token provider (Kotlin app only: Android without the
/// `native` renderer feature). The session page lives in Kotlin, so there is
/// no dioxus runtime and `AUTH_STATE` is unreadable — the JNI bridge parks
/// the captured token here (`notifySession`) and every data fetcher below
/// resolves through it. Single user, single token: a process-wide slot is
/// exact, not a shortcut.
///
/// The slot exists on all Android builds (the bridge compiles for the legacy
/// renderer target too) but only the headless core CONSULTS it — see
/// `ensure_token`. Renderer behavior is therefore identical with or without
/// this module present.
#[cfg(target_os = "android")]
static HEADLESS_TOKEN: std::sync::OnceLock<std::sync::Mutex<(Option<String>, u64)>> =
    std::sync::OnceLock::new();

/// Park a captured token for headless data fetchers. Called by the JNI
/// bridge only.
#[cfg(target_os = "android")]
pub fn set_headless_token(token: String, expires_at_ms: u64) {
    let slot = HEADLESS_TOKEN.get_or_init(|| std::sync::Mutex::new((None, 0)));
    *slot.lock().unwrap_or_else(|e| e.into_inner()) = (Some(token), expires_at_ms);
}

/// Forget the parked token (logout).
#[cfg(target_os = "android")]
pub fn clear_headless_token() {
    if let Some(slot) = HEADLESS_TOKEN.get() {
        *slot.lock().unwrap_or_else(|e| e.into_inner()) = (None, 0);
    }
}

#[cfg(all(target_os = "android", not(feature = "native")))]
fn headless_token() -> Option<String> {
    let (token, expires_ms) = HEADLESS_TOKEN
        .get()?
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let now_ms = Utc::now().timestamp_millis().max(0) as u64;
    match token {
        Some(t) if expires_ms > now_ms + 60_000 => Some(t),
        _ => None,
    }
}

/// Make sure we hold a (reasonably) live access token, refreshing via the
/// hidden SDK WebView when needed. The WebView owns the session cookies, so the
/// refresh is a JS fetch to `open.spotify.com/get_access_token` bridged to Rust.
pub async fn ensure_token() -> Result<String> {
    // Headless core first: no runtime exists there, so the mirror slot below
    // is the ONLY token source (same 60s freshness skew as the runtime path).
    #[cfg(all(target_os = "android", not(feature = "native")))]
    {
        if let Some(token) = headless_token() {
            return Ok(token);
        }
    }

    // Guard: if we're running outside a Dioxus runtime (e.g. store SWR background
    // task, tokio::spawn), we can't access AUTH_STATE at all — return an auth
    // error so the caller can serve stale data or fail gracefully. Headless
    // builds get the typed variant so the bridge maps it to NEEDS_PAGE
    // (revive the login page) instead of a generic auth failure.
    if dioxus::core::Runtime::try_current().is_none() {
        tracing::warn!("session: ensure_token called outside Dioxus runtime");
        #[cfg(all(target_os = "android", not(feature = "native")))]
        return Err(AppError::NoBridgeSession);
        #[cfg(not(all(target_os = "android", not(feature = "native"))))]
        return Err(AppError::Auth("no dioxus runtime available".into()));
    }

    // `peek()` — NOT `.read()`. `ensure_token` runs inside page resources
    // (e.g. Home's `use_resource`), and `.read()` would register `AUTH_STATE` as
    // a reactive dependency of that resource. The session WebView posts a fresh
    // token into `AUTH_STATE` every ~2s, which would cancel and restart any
    // in-flight page fetch forever. `peek()` reads the value without
    // subscribing.
    let (token_opt, expires_ms) = {
        let s = AUTH_STATE.peek();
        (s.access_token.clone(), s.expires_at_ms)
    };
    let now_ms = Utc::now().timestamp_millis() as u64;

    // Token is fresh — return immediately.
    if let Some(token) = token_opt {
        if expires_ms > now_ms + 60_000 {
            return Ok(token);
        }
    }

    // Token expired or missing: ask the hidden SDK WebView to refresh it using
    // the HttpOnly session cookies that only the WebView can present.
    #[cfg(feature = "desktop")]
    {
        let has_token = AUTH_STATE.peek().access_token.is_some();
        tracing::info!(
            "session: token expired, refreshing via hidden WebView (has_token={has_token}, expires_ms={expires_ms}, now_ms={now_ms})",
        );
        let rx = crate::player::webview_bridge::request_token_refresh().await;
        match tokio::time::timeout(std::time::Duration::from_secs(10), rx).await {
            Ok(Ok(Ok(fresh_token))) => {
                // AUTH_STATE was already updated in the IPC handler.
                tracing::info!("session: token refreshed via WebView");
                Ok(fresh_token)
            }
            Ok(Ok(Err(_))) => {
                // Session truly expired — force re-login.
                tracing::warn!("session: refresh reported session expired");
                AUTH_STATE.write().is_authenticated = false;
                Err(AppError::SessionExpired)
            }
            Ok(Err(_)) | Err(_) => {
                // The answer was lost — most often because the IPC drain wasn't
                // up yet when the refresh round-tripped, or the refresh request
                // was superseded. The refresh itself may still have landed in
                // AUTH_STATE via a periodic capture, so re-check before
                // declaring failure.
                let (token_opt, expires_ms) = {
                    let s = AUTH_STATE.peek();
                    (s.access_token.clone(), s.expires_at_ms)
                };
                let now_ms = Utc::now().timestamp_millis() as u64;
                if let Some(token) = token_opt {
                    if expires_ms > now_ms + 60_000 {
                        tracing::info!("session: refresh landed in AUTH_STATE despite lost answer");
                        return Ok(token);
                    }
                }
                tracing::warn!("session: token refresh timed out (10s)");
                Err(anyhow::anyhow!("Token refresh timed out").into())
            }
        }
    }

    // Non-desktop renderers have no hidden WebView to refresh through; a dead
    // token means the login page must be shown again.
    #[cfg(not(feature = "desktop"))]
    {
        AUTH_STATE.write().is_authenticated = false;
        Err(AppError::SessionExpired)
    }
}
