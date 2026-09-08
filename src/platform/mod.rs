//! Cross-platform seams.
//!
//! Native (desktop/mobile) and WASM (web) diverge in ways the rest of the app
//! should not care about; each seam lives in its own module with a per-arch
//! implementation while keeping the native path behaviour-equivalent:
//!
//! * `storage` — native: files under `dirs`; wasm: `localStorage`.
//! * audio sink — native: symphonia + rodio/cpal thread; wasm: browser
//!   `AudioContext` (see `crate::media::sink`).
//! * ad-block DNS — native: `hickory-resolver` DoH; wasm: no DoH.

pub mod storage;

/// Renderer-agnostic access to the native wry window (webview host).
#[cfg(feature = "native")]
pub mod webview;

/// Android-only view-layering for the in-window WebViews (wry 0.53.5 has no
/// multi-WebView support on Android — see `android_views` docstring).
/// Dioxus-mobile-renderer only: it drives wry's view tree through
/// `wry::prelude::dispatch`, which does not exist in the Kotlin app's
/// headless core build (no `native` feature). The Kotlin interface owns its
/// own view layering in `MainActivity`.
#[cfg(all(target_os = "android", feature = "native"))]
pub mod android_views;

/// Android-only platform WebView for the Spotify sign-in/session page. A
/// second wry WebView would permanently clobber the dioxus base view's
/// custom-protocol handlers (see `android_webview` docstring), so the login
/// page lives in a plain `android.webkit.WebView` driven over JNI instead.
/// Same renderer-only scoping as `android_views`: the Kotlin app hosts its
/// login page in `LoginWebView` and never uses this module.
#[cfg(all(target_os = "android", feature = "native"))]
pub mod android_webview;

/// Browser login flow (whole-tab redirect + credentialed token capture).
#[cfg(target_arch = "wasm32")]
pub mod web_login;

/// Spawn a fire-and-forget future. Native uses a tokio task (requires `Send`);
/// wasm's reqwest/fetch futures are `!Send`, so it uses `spawn_local` on the
/// single wasm thread instead. Call sites don't care which runtime hosts it.
#[cfg(not(target_arch = "wasm32"))]
pub fn spawn_background<F>(fut: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    tokio::spawn(fut);
}

/// See the native `spawn_background`; on wasm the future need not be `Send`.
#[cfg(target_arch = "wasm32")]
pub fn spawn_background<F>(fut: F)
where
    F: std::future::Future<Output = ()> + 'static,
{
    wasm_bindgen_futures::spawn_local(fut);
}
