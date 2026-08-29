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
