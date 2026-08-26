/// The HTML+JS bootstrap for the Web Playback SDK. Compiled out of headless
/// (non-desktop) builds, but always available under `cargo test`.
#[cfg(any(feature = "desktop", test))]
pub mod playback_sdk;

/// Desktop builds drive playback through the hidden wry WebView running the
/// Spotify Web Playback SDK. Every other renderer falls back to the Connect API.
#[cfg(feature = "desktop")]
pub mod webview_bridge;

/// PlaybackEngine trait: abstraction over SDK vs open engine.
pub mod engine;

use crate::app_error::AppError;
use crate::state::{AUTH_STATE, PLAYER_STATE};
use dioxus::prelude::ReadableExt;

/// Fire-and-forget playback launch used by every play button in the UI.
pub fn launch(uri: String) {
    dioxus::prelude::spawn(async move {
        if let Err(err) = play_uri(&uri).await {
            if matches!(err, AppError::PremiumRequired(_)) {
                crate::state::publish_error(AppError::PremiumRequired(
                    "Playback requires Spotify Premium — your account can browse freely."
                        .into(),
                ));
            }
        }
    });
}

/// Enqueue a track onto the local play queue (deduplicates by id).
pub fn enqueue(track: crate::spotify::models::Track) {
    PLAYER_STATE.write().enqueue(track);
}

/// Create (once) whichever playback backend this build uses.
pub fn init() -> anyhow::Result<()> {
    #[cfg(feature = "desktop")]
    {
        // A hidden session WebView (open.spotify.com + token-capture) must exist
        // for token refreshes even when no sign-in ran (keychain-restored
        // session). The sign-in flow already leaves one behind.
        crate::auth::webview_login::ensure_session()?;
        webview_bridge::init()
    }
    #[cfg(not(feature = "desktop"))]
    {
        Ok(())
    }
}

/// Tear down the playback backend (used by logout so the SDK releases its
/// cookie jar before the session is cleared).
pub fn shutdown() {
    #[cfg(feature = "desktop")]
    {
        webview_bridge::shutdown();
    }
}

/// Tell the playback backend a session is now available (after a fresh login).
/// Desktop: reconnect the hidden SDK — it fetches its own token from the shared
/// session cookies.
pub fn on_authenticated() {
    #[cfg(feature = "desktop")]
    {
        webview_bridge::reconnect();
    }
}

/// Decide which engine to use based on settings and account state.
fn should_use_open_engine() -> bool {
    use crate::settings::EnginePreference;
    let pref = crate::state::SETTINGS.read().engine;
    match pref {
        EnginePreference::Open => true,
        EnginePreference::SpotifySdk => false,
        EnginePreference::Auto => {
            // Use SDK when Premium; open engine for free accounts.
            !crate::state::AUTH_STATE.read().is_premium()
        }
    }
}

/// Start playing the given Spotify URI.
///
/// Routes to either the SDK (Premium) or the open streaming engine (free/forced)
/// based on `EnginePreference` in Settings.
pub async fn play_uri(uri: &str) -> Result<(), AppError> {
    if should_use_open_engine() {
        return open_play_uri(uri).await;
    }
    // SDK path: requires Premium + device_id.
    if !AUTH_STATE.read().is_premium() {
        return Err(AppError::PremiumRequired(
            "Playback requires Spotify Premium — your account can browse freely.".into(),
        ));
    }
    let device_id = PLAYER_STATE
        .peek()
        .device_id
        .clone()
        .ok_or_else(|| AppError::Playback("no playback device available yet".into()))?;
    crate::spotify::player_api::play(&device_id, uri, None).await
}

/// Play a track via the open streaming engine.
///
/// Extracts the Spotify track ID from the URI, resolves it through the provider
/// chain, and plays the result through the audio sink.
async fn open_play_uri(uri: &str) -> Result<(), AppError> {
    let track_id = uri
        .strip_prefix("spotify:track:")
        .unwrap_or(uri);
    // Look up the track in Spotify's API for metadata.
    let track = crate::spotify::api::get_track(track_id)
        .await
        .map_err(|e| AppError::Playback(format!("failed to fetch track metadata: {e}")))?;
    // Resolve through the open engine.
    let stream = crate::streaming::resolver::resolve(&track)
        .await
        .map_err(|e| AppError::Playback(format!("resolver error: {e}")))?;
    match stream {
        Some(s) => {
            tracing::info!("open engine: playing {} via {}", track.name, s.provider);
            // TODO: send to audio sink (Phase 4b sink wiring)
            Ok(())
        }
        None => Err(AppError::Playback(
            "could not find this track on any provider".into(),
        )),
    }
}

pub async fn play() -> Result<(), AppError> {
    #[cfg(feature = "desktop")]
    {
        webview_bridge::play();
        Ok(())
    }
    #[cfg(not(feature = "desktop"))]
    {
        connect_start(true).await
    }
}

pub async fn pause() -> Result<(), AppError> {
    #[cfg(feature = "desktop")]
    {
        webview_bridge::pause();
        Ok(())
    }
    #[cfg(not(feature = "desktop"))]
    {
        connect_start(false).await
    }
}

pub async fn next() -> Result<(), AppError> {
    // Phase 4: local queue takes precedence over SDK skip.
    let head = PLAYER_STATE.peek().queue_next().cloned();
    if let Some(track) = head {
        PLAYER_STATE.write().pop_queue_head();
        return play_uri(&track.uri).await;
    }
    #[cfg(feature = "desktop")]
    {
        webview_bridge::next();
        Ok(())
    }
    #[cfg(not(feature = "desktop"))]
    {
        connect_skip(true).await
    }
}

pub async fn prev() -> Result<(), AppError> {
    #[cfg(feature = "desktop")]
    {
        webview_bridge::prev();
        Ok(())
    }
    #[cfg(not(feature = "desktop"))]
    {
        connect_skip(false).await
    }
}

pub async fn seek(ms: u64) -> Result<(), AppError> {
    #[cfg(feature = "desktop")]
    {
        webview_bridge::seek(ms);
        Ok(())
    }
    #[cfg(not(feature = "desktop"))]
    {
        let device_id = current_device()?;
        crate::spotify::player_api::seek(&device_id, ms).await
    }
}

pub async fn volume(v: f32) -> Result<(), AppError> {
    #[cfg(feature = "desktop")]
    {
        webview_bridge::volume(v);
        Ok(())
    }
    #[cfg(not(feature = "desktop"))]
    {
        let device_id = current_device()?;
        crate::spotify::player_api::set_volume(&device_id, (v.clamp(0.0, 1.0) * 100.0) as u8)
            .await
    }
}

#[cfg(not(feature = "desktop"))]
fn current_device() -> Result<String, AppError> {
    PLAYER_STATE
        .peek()
        .device_id
        .clone()
        .ok_or_else(|| AppError::Playback("no playback device available yet".into()))
}

#[cfg(not(feature = "desktop"))]
async fn connect_start(play: bool) -> Result<(), AppError> {
    let device_id = current_device()?;
    if play {
        crate::spotify::player_api::play(&device_id, "", None).await
    } else {
        crate::spotify::player_api::pause(&device_id).await
    }
}

#[cfg(not(feature = "desktop"))]
async fn connect_skip(next: bool) -> Result<(), AppError> {
    let device_id = current_device()?;
    crate::spotify::player_api::skip(&device_id, next).await
}