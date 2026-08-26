//! PlaybackEngine trait: abstraction over SDK vs open engine.
//!
//! The trait lets the player dispatch (`player/mod.rs`) call playback
//! operations without knowing which backend is active. The Settings page
//! selects the engine; the `auto` mode picks SDK when Premium is available
//! and falls back to the open engine otherwise.

use crate::app_error::AppError;

/// The result of starting playback on a track.
pub enum PlayResult {
    /// Playback started successfully.
    Ok,
    /// The engine needs the user to authenticate.
    AuthRequired,
    /// The engine encountered a fatal error.
    Error(AppError),
}

/// A playback engine that can drive audio output.
///
/// Implementations:
/// - `SdkEngine` (desktop): wraps the hidden WebView + Web Playback SDK.
/// - `OpenEngine`: wraps the open streaming engine + rodio sink.
pub trait PlaybackEngine: Send + Sync {
    /// Human-readable name for logging.
    fn name(&self) -> &'static str;

    /// Start playing a Spotify track URI.
    fn play_uri(&self, uri: &str) -> impl std::future::Future<Output = Result<(), AppError>> + Send;

    /// Pause playback.
    fn pause(&self) -> impl std::future::Future<Output = Result<(), AppError>> + Send;

    /// Resume playback.
    fn play(&self) -> impl std::future::Future<Output = Result<(), AppError>> + Send;

    /// Skip to the next track.
    fn next(&self) -> impl std::future::Future<Output = Result<(), AppError>> + Send;

    /// Skip to the previous track.
    fn prev(&self) -> impl std::future::Future<Output = Result<(), AppError>> + Send;

    /// Seek to a position in milliseconds.
    fn seek(&self, ms: u64) -> impl std::future::Future<Output = Result<(), AppError>> + Send;

    /// Set volume (0.0–1.0).
    fn volume(&self, v: f32) -> impl std::future::Future<Output = Result<(), AppError>> + Send;

    /// Whether this engine is available right now.
    fn is_available(&self) -> bool;
}
