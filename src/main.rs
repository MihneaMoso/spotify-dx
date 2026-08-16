//! Spotify DX — a cross-platform Spotify client that hides premium previews.
//!
//! Renderers: `--features desktop` (primary), `web` and `mobile`. On desktop we
//! boot a hidden wry WebView that runs the Web Playback SDK plus a file-based
//! fetch routing engine; on web/mobile everything falls back to the Connect API.

#![forbid(unsafe_code)]

pub mod adblock;
pub mod app;
pub mod app_error;
pub mod auth;
pub mod player;
pub mod spotify;
pub mod state;
pub mod ui;
pub mod util;

/// Shared startup: tracing, ad-blocker, auth boot, then hand the session to the
/// renderer. `bootstrap` runs inside a tokio runtime; it never touches dioxus.
async fn bootstrap() {
    if let Err(err) = adblock::init().await {
        tracing::warn!("adblock: bootstrap failed ({err:#}); continuing without a blocker");
    }
    match auth::init().await {
        Ok(Some(session)) => auth::set_boot_auth(session),
        Ok(None) => tracing::info!("auth: no stored session; will show the login gate"),
        Err(err) => tracing::warn!("auth: bootstrap could not restore a session: {err:#}"),
    }
}

fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::new(
        std::env::var("SPOTIFY_DX_LOG")
            .unwrap_or_else(|_| "rspotify=warn,wry=warn,tao=warn,tokio=info,spotify_dx=info".into()),
    );
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

#[cfg(feature = "desktop")]
fn main() {
    use app::App;
    use dioxus::desktop::{Config, LogicalSize, WindowBuilder};

    init_logging();

    // Everything that reads network/token/blocklist data happens before the
    // window is mounted so the first frame is instant.
    let rt = tokio::runtime::Runtime::new().expect("failed to start the tokio runtime");
    rt.block_on(bootstrap());
    drop(rt);

    let window = WindowBuilder::new()
        .with_title("Spotify DX")
        .with_inner_size(LogicalSize::new(1200.0, 780.0))
        .with_min_inner_size(LogicalSize::new(400.0, 600.0))
        // Frameless: hides the GTK title bar ("Spotify DX" + window buttons)
        // so the webview is the whole window. The app renders its own chrome.
        .with_decorations(false);

    let config = Config::new()
        .with_window(window)
        // No native menu bar: this app is a pure webview shell, so an OS menu
        // would only duplicate/replace the in-app chrome.
        .with_menu(None)
        .with_disable_context_menu(true);
    dioxus::LaunchBuilder::desktop().with_cfg(config).launch(App);
}

/// Other renderers fall back to the Connect API (no local WebView).
#[cfg(all(not(feature = "desktop"), feature = "web"))]
fn main() {
    use app::App;
    init_logging();
    dioxus::launch(App);
}

#[cfg(all(not(feature = "desktop"), feature = "mobile"))]
fn main() {
    use app::App;
    init_logging();
    dioxus::launch(App);
}

/// Headless / tooling build (CI, tests, `cargo check` without renderers): just
/// exercise the bootstrap path and exit.
#[cfg(not(any(feature = "desktop", feature = "web", feature = "mobile")))]
fn main() {
    init_logging();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to start the tokio runtime");
    rt.block_on(bootstrap());

    let session = auth::take_boot_auth();
    println!(
        "spotify-dx headless (tooling build): adblock={} auth={}",
        if crate::state::is_blocker_ready() { "ready" } else { "not-ready" },
        if session.is_some() { "restored" } else { "none" }
    );
}