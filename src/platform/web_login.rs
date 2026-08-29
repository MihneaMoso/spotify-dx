//! Browser login flow (wasm only).
//!
//! The desktop build hosts `open.spotify.com` in a hidden WebView and captures a
//! session there. A browser tab can't host that WebView, so web parity uses the
//! browser itself as the session holder instead:
//!
//! * `redirect_to_spotify` — takes the whole tab to `open.spotify.com` so the
//!   user can sign in there (the app tab is where the browser keeps the
//!   `sp_dc` HttpOnly cookie afte logging in).
//! * `capture_session` — fetches Spotify's internal `get_access_token` endpoint
//!   with `credentials: include`, the same way the desktop WebView's `fetchAccessToken`
//!   does. This relies on Spotify permitting a credentialed cross-origin fetch
//!   from the app's origin and on the browser sending the `sp_dc` session cookie;
//!   when either is denied it returns `Ok(None)` (not logged in) or `Err`
//!   (network / CORS), which the login gate surfaces.
//!
//! This module is compiled only for `wasm32`; native builds never see it.

use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::{Request, RequestInit, RequestMode, Window};

/// Redirect the entire tab to the Spotify sign-in page.
pub fn redirect_to_spotify() {
    let window: Window = web_sys::window().expect("no window in wasm");
    window
        .location()
        .set_href("https://open.spotify.com/")
        .expect("failed to set window.location");
}

/// Try to capture a live web-player session from the browser's Spotify cookies.
///
/// Returns `Ok(Some((access_token, expires_at_ms)))` when a session is present,
/// `Ok(None)` when the user is not signed in, and `Err` when the credentialed
/// fetch is blocked (CORS / third-party-cookie policy) or the network failed.
pub async fn capture_session() -> anyhow::Result<Option<(String, u64)>> {
    let window: Window = web_sys::window().expect("no window in wasm");

    let init = RequestInit::new();
    init.set_method("GET");
    // Send the `sp_dc` HttpOnly session cookie, matching the desktop WebView's
    // `fetchAccessToken` (`credentials: 'include'`).
    init.set_credentials(web_sys::RequestCredentials::Include);
    init.set_mode(RequestMode::Cors);

    let request = Request::new_with_str_and_init(
        "https://open.spotify.com/get_access_token?reason=transport&productType=web_player",
        &init,
    )
    .map_err(|e| anyhow::anyhow!("build request: {e:?}"))?;

    let resp_promise = window.fetch_with_request(&request);
    let resp: web_sys::Response =
        JsFuture::from(resp_promise).await.map_err(|e| anyhow::anyhow!("fetch error: {e:?}"))?
            .dyn_into()
            .map_err(|e| anyhow::anyhow!("cast response: {e:?}"))?;

    if !resp.ok() {
        // 401 => not signed in at open.spotify.com in this browser.
        if resp.status() == 401 {
            return Ok(None);
        }
        return Err(anyhow::anyhow!("get_access_token returned {}", resp.status()));
    }

    let text_promise = resp.text().map_err(|e| anyhow::anyhow!("read body: {e:?}"))?;
    let text: String = JsFuture::from(text_promise)
        .await
        .map_err(|e| anyhow::anyhow!("body future: {e:?}"))?
        .as_string()
        .ok_or_else(|| anyhow::anyhow!("non-text token response"))?;

    let v: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| anyhow::anyhow!("parse token JSON: {e}"))?;

    if v.get("isAnonymous").and_then(|b| b.as_bool()) == Some(true) {
        return Ok(None);
    }
    let token = v
        .get("accessToken")
        .and_then(|t| t.as_str())
        .ok_or_else(|| anyhow::anyhow!("no accessToken in response"))?
        .to_string();
    let expires = v
        .get("accessTokenExpirationTimestampMs")
        .and_then(|e| e.as_u64())
        .ok_or_else(|| anyhow::anyhow!("no expiry in response"))?;

    Ok(Some((token, expires)))
}
