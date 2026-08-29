//! Spotify DX — a cross-platform Spotify client that hides premium previews.
//!
//! Renderers: `--features desktop` (primary), `web` and `mobile`. On desktop we
//! boot a hidden wry WebView that runs the Web Playback SDK; it shares its data
//! directory (session cookies) with the visible login WebView from `auth`.

#![forbid(unsafe_code)]

pub mod adblock;
pub mod app;
pub mod app_error;
pub mod auth;
pub mod media;
pub mod player;
pub mod platform;
pub mod settings;
pub mod spotify;
pub mod state;
pub mod streaming;
pub mod ui;
pub mod util;

/// Shared startup: tracing, ad-blocker, auth boot. Returns `true` when a valid
/// session was restored from the keychain (main UI launches straight away).
async fn bootstrap() -> bool {
    if let Err(err) = adblock::init().await {
        tracing::warn!("adblock: bootstrap failed ({err:#}); continuing without a blocker");
    }
    crate::auth::init().await
}

fn init_logging() {
    // `std::env::var` does not exist on wasm (no process environment), so the
    // log filter is fixed there.
    #[cfg(not(target_arch = "wasm32"))]
    let filter = tracing_subscriber::EnvFilter::new(
        std::env::var("SPOTIFY_DX_LOG")
            .unwrap_or_else(|_| "wry=warn,tao=warn,tokio=info,spotify_dx=info".into()),
    );
    #[cfg(target_arch = "wasm32")]
    let filter = tracing_subscriber::EnvFilter::new("tokio=info,spotify_dx=info");
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

#[cfg(feature = "desktop")]
fn main() {
    use app::App;
    use dioxus::desktop::{Config, LogicalSize, WindowBuilder};

    init_logging();

    // Everything that reads network/token/blocklist data happens before the
    // window is mounted so the first frame is instant. Global signals cannot be
    // touched before the dioxus runtime exists, so auth::init() only inspects
    // the token store; the login gate (which always shows the open.spotify.com
    // web-session WebView) decides the session.
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

/// Mobile renderer: the full app (bootstrap + SDK/open-engine playback via the
/// shared native path). `mobile` is dioxus-desktop under the hood, so it uses
/// the same multi-threaded tokio bootstrap as desktop.
#[cfg(all(not(feature = "desktop"), not(feature = "web"), feature = "mobile"))]
fn main() {
    use app::App;
    init_logging();

    let rt = tokio::runtime::Runtime::new().expect("failed to start the tokio runtime");
    rt.block_on(bootstrap());
    drop(rt);

    dioxus::launch(App);
}

/// Web renderer: run the full web-parity startup (adblock + auth session init)
/// concurrently with mounting the app. On wasm there is no multi-threaded runtime
/// or blocking executor, so bootstrap runs as a `spawn_local` background task.
#[cfg(all(not(feature = "desktop"), feature = "web"))]
fn main() {
    use app::App;
    init_logging();
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        let restored = bootstrap().await;
        tracing::info!("web: bootstrap complete, session restored={restored}");
    });
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
    let has_session = rt.block_on(bootstrap());

    println!(
        "spotify-dx headless (tooling build): adblock={} auth={}",
        if crate::state::is_blocker_ready() { "ready" } else { "not-ready" },
        if has_session { "restored" } else { "none" }
    );
}