/// The HTML+JS bootstrap for the Web Playback SDK. Compiled out of headless
/// (non-desktop) builds, but always available under `cargo test`.
#[cfg(any(feature = "desktop", test))]
pub mod playback_sdk;

/// Desktop builds drive playback through the hidden wry WebView running the
/// Spotify Web Playback SDK. Every other renderer falls back to the Connect API.
#[cfg(feature = "desktop")]
pub mod webview_bridge;

use crate::app_error::AppError;
use crate::state::PLAYER_STATE;
use dioxus::prelude::ReadableExt;

/// Fire-and-forget playback launch used by every play button in the UI.
pub fn launch(uri: String) {
    dioxus::prelude::spawn(async move {
        let _ = play_uri(&uri).await;
    });
}

/// Create (once) whichever playback backend this build uses.
pub fn init() -> anyhow::Result<()> {
    #[cfg(feature = "desktop")]
    {
        webview_bridge::init()
    }
    #[cfg(not(feature = "desktop"))]
    {
        Ok(())
    }
}

/// Tell the playback backend a session is now available (after a fresh login).
/// Desktop: hand the new token to the hidden SDK and reconnect it.
pub fn on_authenticated() {
    #[cfg(feature = "desktop")]
    {
        webview_bridge::reconnect();
    }
}

/// Start playing the given Spotify URI on the active device.
///
/// This path goes through the Connect API regardless of renderer, because the
/// initialized device was reported by the Web Playback SDK.
pub async fn play_uri(uri: &str) -> Result<(), AppError> {
    let device_id = PLAYER_STATE
        .peek()
        .device_id
        .clone()
        .ok_or_else(|| AppError::Playback("no playback device available yet".into()))?;
    crate::spotify::player_api::play(&device_id, uri, None).await
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