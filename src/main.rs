//! Spotify DX — renderer entry points (thin shell over the native core).
//!
//! All services live in the `spotify_dx` library crate (`src/lib.rs`, Phase 0
//! of `docs/KOTLIN_MIGRATION.md`); this binary keeps the per-renderer entry
//! points (`desktop`/`web`/`mobile`/headless) plus the shared `bootstrap()`.
//! The Kotlin Android app (`android/`) links the library directly and does not
//! use this binary at all.

#![forbid(unsafe_code)]

use spotify_dx::adblock;
#[cfg(any(feature = "desktop", feature = "web", feature = "mobile"))]
use spotify_dx::app;
#[cfg(feature = "desktop")]
use spotify_dx::updater;

/// Shared startup: tracing, ad-blocker, auth boot. Returns `true` when a valid
/// session was restored from the keychain (main UI launches straight away).
///
/// NOTE: this runs BEFORE the dioxus runtime exists, so it must not touch any
/// `GlobalSignal` — the profile load (`profile::init`) therefore happens in the
/// `App` component, which runs inside the runtime.
async fn bootstrap() -> bool {
    if let Err(err) = adblock::init().await {
        tracing::warn!("adblock: bootstrap failed ({err:#}); continuing without a blocker");
    }
    spotify_dx::auth::init().await
}

#[cfg(target_arch = "wasm32")]
fn init_logging() {
    // `std::env::var` does not exist on wasm (no process environment), so the
    // log filter is fixed there. IMPORTANT: wasm32-unknown-unknown has no
    // `std::time::SystemTime`, so tracing's default fmt timer would panic on
    // the first event ("time not implemented on this platform") and abort the
    // whole app. We must drop the timestamp on wasm.
    let filter = tracing_subscriber::EnvFilter::new("tokio=info,spotify_dx=info");
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .without_time()
        .init();
}

#[cfg(not(target_arch = "wasm32"))]
fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::new(
        std::env::var("SPOTIFY_DX_LOG")
            .unwrap_or_else(|_| "wry=warn,tao=warn,tokio=info,spotify_dx=info".into()),
    );
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

#[cfg(feature = "desktop")]
fn main() {
    use dioxus::desktop::{Config, LogicalSize, WindowBuilder};

    init_logging();

    // Swap in a staged update (downloaded in a previous session) before the
    // window boots, so the new binary is what ends up running. On success the
    // relaunched process re-runs this code as the new version; on failure we
    // simply continue. No signals are touched here — the dioxus runtime does
    // not exist yet.
    #[cfg(all(not(target_os = "android"), not(target_arch = "wasm32")))]
    {
        let _ = updater::apply_staged_update();
    }

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
    dioxus::LaunchBuilder::desktop().with_cfg(config).launch(app::App);
}

/// Mobile renderer: the full app (bootstrap + SDK/open-engine playback via the
/// shared native path). `mobile` is dioxus-desktop under the hood, so it uses
/// the same multi-threaded tokio bootstrap as desktop.
#[cfg(all(not(feature = "desktop"), not(feature = "web"), feature = "mobile"))]
fn main() {
    init_logging();

    let rt = tokio::runtime::Runtime::new().expect("failed to start the tokio runtime");
    rt.block_on(bootstrap());
    drop(rt);

    dioxus::launch(app::App);
}

/// Web renderer: run the full web-parity startup (adblock + auth session init)
/// concurrently with mounting the app. On wasm there is no multi-threaded runtime
/// or blocking executor, so bootstrap runs as a `spawn_local` background task.
#[cfg(all(not(feature = "desktop"), feature = "web"))]
fn main() {
    init_logging();
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        let restored = bootstrap().await;
        tracing::info!("web: bootstrap complete, session restored={restored}");
    });
    dioxus::launch(app::App);
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
        if spotify_dx::state::is_blocker_ready() { "ready" } else { "not-ready" },
        if has_session { "restored" } else { "none" }
    );
}
