use dioxus::prelude::*;

use crate::app_error::AppError;
use crate::spotify::models::*;
use chrono::{DateTime, Utc};

/// The page currently visible in the UI router.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[derive(Default)]
pub enum Page {
    #[default]
    Home,
    Search,
    Library,
    Album,
    Artist,
    Playlist,
    NowPlaying,
}


/// Playback repeat mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RepeatMode {
    #[default]
    Off,
    Context,
    Track,
}

/// Global application state that is *not* tied to one screen.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AppState {
    pub current_page: Page,
    pub search_query: String,
    pub search_results: Option<SearchResults>,
    pub home_featured: Vec<Playlist>,
    pub home_new_releases: Vec<Album>,
}

/// Flat UI snapshot backing the Search page.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SearchUiState {
    pub tracks: Vec<Track>,
    pub query: String,
    pub has_searched: bool,
    pub has_errors: bool,
}

impl SearchUiState {
    pub fn reset(&mut self) {
        self.tracks.clear();
        self.query.clear();
        self.has_searched = false;
        self.has_errors = false;
    }
}

/// Everything related to the currently playing item.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PlayerState {
    pub track: Option<Track>,
    pub queue: Vec<Track>,
    pub is_playing: bool,
    pub position_ms: u64,
    pub duration_ms: u64,
    pub volume: f32, // 0.0–1.0
    pub shuffle: bool,
    pub repeat: RepeatMode,
    pub device_id: Option<String>, // Spotify Connect device id reported by the hidden WebView
}

/// OAuth / session data.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AuthState {
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub user_id: Option<String>,
    pub user_display_name: Option<String>,
    pub user_avatar_url: Option<String>,
}

impl AuthState {
    /// Whether we hold an access token that has not expired yet.
    pub fn is_authenticated(&self) -> bool {
        if self.access_token.is_none() {
            return false;
        }
        match self.expires_at {
            Some(exp) => Utc::now() < exp,
            None => false,
        }
    }

    /// Replace this state with another session (used after login and boot).
    pub fn copy_from(&mut self, other: &AuthState) {
        self.access_token = other.access_token.clone();
        self.refresh_token = other.refresh_token.clone();
        self.expires_at = other.expires_at;
        self.user_id = other.user_id.clone();
        self.user_display_name = other.user_display_name.clone();
        self.user_avatar_url = other.user_avatar_url.clone();
    }

    /// Drop all session data (used by logout and account reset).
    pub fn reset(&mut self) {
        self.access_token = None;
        self.refresh_token = None;
        self.expires_at = None;
        self.user_id = None;
        self.user_display_name = None;
        self.user_avatar_url = None;
    }
}

/// Numbers exposed in the "ad-block" status panel.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct AdblockStats {
    /// Hostnames currently held in the block tree.
    pub tracked: usize,
    /// Requests dropped since startup.
    pub blocked: usize,
    /// Cache entries loaded from `assets/blocklist_cache.txt`.
    pub cached_entries: usize,
    /// Times the upstream blocklist failed to refresh.
    pub ad_fetch_failures: u64,
}

// ── Global signals: the single source of truth for the whole app ──

pub static APP_STATE: GlobalSignal<AppState> = Signal::global(AppState::default);
pub static PLAYER_STATE: GlobalSignal<PlayerState> = Signal::global(PlayerState::default);
pub static AUTH_STATE: GlobalSignal<AuthState> = Signal::global(AuthState::default);
pub static ADBLOCK_STATS: GlobalSignal<AdblockStats> = Signal::global(AdblockStats::default);

/// The most recent application-level error, consumed by the toast component.
pub static APP_ERROR: GlobalSignal<Option<AppError>> = Signal::global(|| None);

/// Whether the ad filter finished seeding its rule tree.
pub fn is_blocker_ready() -> bool {
    crate::adblock::is_ready()
}

/// Publish an error that the toast component will render and clear.
pub fn publish_error(err: AppError) {
    APP_ERROR.write().replace(err);
}

/// Clear the active error toast.
pub fn clear_error() {
    APP_ERROR.write().take();
}