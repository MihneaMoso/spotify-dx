use dioxus::prelude::*;

use crate::app_error::AppError;
use crate::spotify::models::*;

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

/// Web-session auth data. The access token comes from Spotify's internal
/// web-player endpoint (`open.spotify.com/get_access_token`) — it is short-lived
/// (~1h) and refreshed via the hidden SDK WebView. No refresh_token exists:
/// session cookies live in the shared WebView data directory, never in Rust.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AuthState {
    pub access_token: Option<String>,
    /// Unix epoch in milliseconds — the expiry time of the access token.
    pub expires_at_ms: u64,
    pub user_id: Option<String>,
    pub user_display_name: Option<String>,
    pub user_avatar_url: Option<String>,
    /// "premium" | "free" | None (unknown)
    pub product: Option<String>,
    pub is_authenticated: bool,
}

impl AuthState {
    /// Whether the account tier allows Web Playback SDK playback.
    pub fn is_premium(&self) -> bool {
        self.product.as_deref() == Some("premium")
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