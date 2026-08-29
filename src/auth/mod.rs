use crate::state::{AUTH_STATE, AuthState};
pub mod token_store;

/// Desktop-only: hosts the real `open.spotify.com` sign-in in the main window.
#[cfg(all(feature = "desktop", target_os = "linux"))]
pub mod webview_login;

#[cfg(feature = "desktop")]
use std::cell::RefCell;

#[cfg(all(feature = "desktop", not(target_os = "linux")))]
pub mod webview_login {
    use super::WebSessionResult;

    pub fn start(tx: tokio::sync::oneshot::Sender<WebSessionResult>) -> anyhow::Result<()> {
        let _ = tx;
        anyhow::bail!("desktop in-window login webview is currently only supported on Linux")
    }

    pub fn ensure_session() -> anyhow::Result<()> {
        Ok(())
    }

    pub fn hide() {}

    pub fn shutdown() {}

    pub async fn refresh_token() -> bool {
        false
    }
}

/// The one WebView data directory shared by the login WebView AND the hidden
/// SDK WebView. Cookies and localStorage persist here.
pub fn webview_data_dir() -> std::path::PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("spotify-dx")
        .join("webview_session")
}

// The single session `WebContext`, shared by the sign-in WebView and the
// hidden SDK WebView for the whole process. webkitgtk aborts when a second
// `WebContext` claims a data directory still held by a live (cached) web
// process, so two contexts on one directory — as the old login-window design
// did — is a hard crash.
#[cfg(feature = "desktop")]
thread_local! {
    static SESSION_CONTEXT: RefCell<Option<wry::WebContext>> = const { RefCell::new(None) };
}

/// Run `f` with the process-wide session `WebContext`, creating it on first
/// use. Both WebViews are built with this context, so the cookies written
/// during login are exactly what the hidden SDK WebView sees later.
///
/// The borrow only lives for the duration of the closure (the guard cannot
/// escape a `thread_local`'s `.with`), so build the WebView inside the closure
/// and return it from `f`.
#[cfg(feature = "desktop")]
pub fn with_session_context<R>(f: impl FnOnce(&mut wry::WebContext) -> R) -> R {
    SESSION_CONTEXT.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = Some(wry::WebContext::new(Some(webview_data_dir())));
        }
        f(slot.as_mut().expect("session context just created"))
    })
}

/// The token handed back by the WebView after a successful login (or refresh).
#[derive(Debug)]
pub struct WebSessionResult {
    pub access_token: String,
    pub expires_at_ms: u64,
    pub is_anonymous: bool,
}

/// Waits for the IPC result from the login WebView.
pub async fn await_session(
    rx: tokio::sync::oneshot::Receiver<WebSessionResult>,
) -> anyhow::Result<WebSessionResult> {
    rx.await
        .map_err(|_| anyhow::anyhow!("Login window closed before session captured"))
}

/// Desktop: open the Spotify sign-in window, capture the web-player session,
/// persist it and flip `AUTH_STATE` to authenticated. The login WebView writes
/// the HttpOnly session cookies into the shared data directory, so once the
/// token is stored here the user stays logged in on future launches.
#[cfg(feature = "desktop")]
pub async fn login() -> anyhow::Result<()> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    webview_login::start(tx)?;
    let session = await_session(rx).await?;
    // Keep the WebView alive (hidden) as the session WebView: it is the only
    // same-origin WebView whose get_access_token fetch works, so all token
    // refreshes are routed through it (see player/webview_bridge.rs).
    webview_login::hide();

    if session.access_token.is_empty() {
        // JS detected the logged-in page but could not capture a token
        // (`get_access_token` has been flaky since Spotify tightened it in
        // 2025). The hidden SDK WebView shares the session cookies and will
        // fetch a token on its own — just swap to the main UI; App backfills
        // the profile from within the dioxus runtime once the SDK has booted.
        tracing::info!("login: proceeding without a captured token");
        AUTH_STATE.write().is_authenticated = true;
        return Ok(());
    }

    on_session_captured(session.access_token, session.expires_at_ms).await
}

/// Non-desktop renderers have no WebView to host the login page.
#[cfg(not(feature = "desktop"))]
pub async fn login() -> anyhow::Result<()> {
    anyhow::bail!("this build cannot open the Spotify login window")
}

/// Called at app startup. Reports whether a clock-valid session exists in the
/// keychain.
///
/// This runs in `main.rs` *before* the dioxus runtime exists, so it must not
/// touch runtime-dependent state (global signals) — it only inspects the token
/// store. The UI still shows the web-session WebView (`open.spotify.com`) at
/// startup: that page is the source of truth, and tokens are refreshed through
/// it, so a stored token never skips the login gate.
pub async fn init() -> bool {
    if let Some((_, expires_ms)) = token_store::load() {
        let now_ms = chrono::Utc::now().timestamp_millis() as u64;
        if expires_ms > now_ms + 60_000 {
            return true;
        }
    }
    false
}

/// Called by the UI after the login WebView IPC delivers a session: persist the
/// token, write AUTH_STATE, fetch the profile synchronously so the state is
/// complete before the caller flips the UI to the main router.
pub async fn on_session_captured(token: String, expires_at_ms: u64) -> anyhow::Result<()> {
    token_store::save(&token, expires_at_ms);
    {
        let mut state = AUTH_STATE.write();
        state.access_token = Some(token);
        state.expires_at_ms = expires_at_ms;
    }
    refresh_profile().await;
    {
        let mut state = AUTH_STATE.write();
        state.is_authenticated = true;
    }
    Ok(())
}

/// Forget the session: clear the keychain + fallback file and reset state.
pub fn logout() {
    token_store::clear();
    *AUTH_STATE.write() = AuthState::default();
    #[cfg(feature = "desktop")]
    {
        // Drop the cookie-holding WebViews so a fresh login starts clean.
        crate::player::shutdown();
        webview_login::shutdown();
    }
}

/// Fetch the /v1/me profile and fold it into AUTH_STATE.
pub async fn refresh_profile() {
    let Ok(profile) = crate::spotify::api::get_current_user_profile().await else {
        return;
    };
    let mut s = AUTH_STATE.write();
    s.user_id = Some(profile.id);
    s.user_display_name = profile.display_name;
    s.user_avatar_url = profile.images.into_iter().next().map(|i| i.url);
    s.product = profile.product;
    s.is_authenticated = true;
}
