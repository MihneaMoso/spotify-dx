//! Renderer-agnostic access to the native wry window (webview host).
//!
//! `dioxus::desktop` and `dioxus::mobile` are the same `dioxus_desktop` crate
//! re-exported under different names (mobile re-exports desktop wholesale). But
//! each `dioxus::X` module only exists when the matching `dioxus/X` feature is
//! on, so code shared by both renderers cannot name `dioxus::desktop` for a pure
//! mobile build. This module picks the right alias per active renderer so
//! `webview_bridge` (SDK + IPC) can build its hidden WebView on every native
//! platform.
//!
//! Note `--features mobile` keeps the default `desktop` feature too (default =
//! `["desktop"]`), so both aliases can be active at once. They are the same
//! crate; we just prefer `mobile` for the type/`window()` alias to keep a single
//! definition.

/// The wry window handle + dioxus webview, as exposed by whichever native
/// renderer is compiled in.
#[cfg(feature = "mobile")]
pub use dioxus::mobile::DesktopContext as WindowContext;
#[cfg(all(feature = "desktop", not(feature = "mobile")))]
pub use dioxus::desktop::DesktopContext as WindowContext;

/// The current dioxus window context.
#[cfg(feature = "mobile")]
pub fn window() -> WindowContext {
    dioxus::mobile::window()
}
#[cfg(all(feature = "desktop", not(feature = "mobile")))]
pub fn window() -> WindowContext {
    dioxus::desktop::window()
}

/// The GTK container webviews are packed into (Linux desktop only). Mobile and
/// Wry's non-Linux backend use the cross-platform `WebViewBuilder::build(&window)`
/// API instead.
#[cfg(all(feature = "desktop", target_os = "linux"))]
pub fn default_vbox() -> anyhow::Result<gtk::Box> {
    use dioxus::desktop::tao::platform::unix::WindowExtUnix;
    let desktop = window();
    let vbox = desktop
        .window
        .default_vbox()
        .ok_or_else(|| anyhow::anyhow!("main window has no gtk container for the webview"))?;
    Ok(vbox.clone())
}

/// The window's logical client size, as `(width, height)`. Used by the
/// cross-platform login WebView (mobile / non-GTK desktop) so it can fill the
/// whole window the way the Linux desktop path fills its GTK container. Same
/// `dioxus::desktop`/`dioxus::mobile` alias rule as `window()`.
#[cfg(not(all(feature = "desktop", target_os = "linux")))]
pub fn window_logical_size() -> (f64, f64) {
    let desktop = window();
    let scale = desktop.window.scale_factor();
    let size = desktop.window.inner_size();
    (size.width as f64 / scale, size.height as f64 / scale)
}
