use thiserror::Error;

/// Typed application error with variants the UI can surface to the user.
#[derive(Debug, Error)]
pub enum AppError {
    #[error("authentication failed: {0}")]
    Auth(String),
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("request to {0} was dropped by the ad filter")]
    AdBlock(String),
    #[error("playback error: {0}")]
    Playback(String),
    #[error("spotify api error: {0}")]
    Spotify(String),
    #[cfg(feature = "desktop")]
    #[error("webview error: {0}")]
    Webview(String),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}