//! Provider trait and resolution states.
//!
//! Each music source (TIDAL, Qobuz, YouTube, …) implements [`Provider`].
//! The resolver tries providers in order and uses the explicit [`Resolution`]
//! state to decide whether to fail over, cooldown, or return success.

use std::fmt;

/// Explicit result of asking a provider for a stream URL.
///
/// This is the core of the failover design: every provider must return one of
/// these four states so the resolver can make an informed decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// Provider returned a playable URL.
    Success {
        url: String,
        format: AudioFormat,
        quality: Quality,
    },
    /// Provider is temporarily unavailable (503 / rate-limited).
    /// `retry_after_secs` is the minimum wait before retrying this provider.
    Cooldown {
        retry_after_secs: u64,
    },
    /// Track not found on this provider (wrong region, missing catalog).
    NotFound,
    /// Unexpected error (network failure, parse error, etc.).
    Error(String),
}

impl Resolution {
    pub fn is_success(&self) -> bool {
        matches!(self, Resolution::Success { .. })
    }
}

/// Audio container format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AudioFormat {
    Flac,
    Mp3,
    Aac,
    Ogg,
    Opus,
    Unknown,
}

impl AudioFormat {
    /// File extension for format hinting.
    pub fn extension(&self) -> &'static str {
        match self {
            AudioFormat::Flac => "flac",
            AudioFormat::Mp3 => "mp3",
            AudioFormat::Aac => "m4a",
            AudioFormat::Ogg => "ogg",
            AudioFormat::Opus => "opus",
            AudioFormat::Unknown => "",
        }
    }
}

impl fmt::Display for AudioFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.extension())
    }
}

/// Audio quality tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Quality {
    Low,
    Normal,
    High,
    Lossless,
}

impl fmt::Display for Quality {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Quality::Low => write!(f, "low"),
            Quality::Normal => write!(f, "normal"),
            Quality::High => write!(f, "high"),
            Quality::Lossless => write!(f, "lossless"),
        }
    }
}

/// Track metadata needed by providers to locate a stream.
/// Providers use different subsets (TIDAL uses Spotify ID, Qobuz uses ISRC,
/// YouTube uses artist+title).
#[derive(Debug, Clone, Default)]
pub struct TrackQuery {
    /// Spotify track ID (e.g. "4PTG3Z6ehGkBFwjybBmmQ").
    pub spotify_id: String,
    /// International Standard Recording Code, if available from the album.
    pub isrc: Option<String>,
    /// Track name.
    pub title: String,
    /// Primary artist name.
    pub artist: String,
    /// Album name (used by some providers for disambiguation).
    pub album: Option<String>,
    /// Track duration in milliseconds.
    pub duration_ms: u64,
}

/// A music source that can resolve a track query into a stream URL.
///
/// Providers are registered in priority order. The resolver tries each one;
/// on `NotFound` or `Error` it moves to the next; on `Cooldown` it either
/// skips or waits depending on the cooldown duration.
#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    /// Human-readable name for logging (e.g. "tidal", "qobuz").
    fn name(&self) -> &'static str;

    /// Check if this provider is likely available (uptime list says so, etc.).
    /// Defaults to `true`; providers with uptime lists override this.
    fn is_available(&self) -> bool {
        true
    }

    /// Resolve a track query into a playable stream URL.
    async fn resolve(&self, query: &TrackQuery) -> Resolution;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolution_is_success() {
        assert!(Resolution::Success {
            url: "https://example.com/stream.flac".into(),
            format: AudioFormat::Flac,
            quality: Quality::Lossless,
        }
        .is_success());
        assert!(!Resolution::NotFound.is_success());
        assert!(!Resolution::Error("boom".into()).is_success());
        assert!(!Resolution::Cooldown { retry_after_secs: 30 }.is_success());
    }

    #[test]
    fn audio_format_extension() {
        assert_eq!(AudioFormat::Flac.extension(), "flac");
        assert_eq!(AudioFormat::Unknown.extension(), "");
    }

    #[test]
    fn quality_ordering() {
        assert!(Quality::Low < Quality::Normal);
        assert!(Quality::High < Quality::Lossless);
    }
}
