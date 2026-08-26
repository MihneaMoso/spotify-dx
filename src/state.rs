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
    /// Snapshot of `queue` ids taken when local shuffle is enabled, so toggling
    /// shuffle off can restore the original order. Empty == "not captured".
    pub queue_original: Vec<String>,
    pub is_playing: bool,
    pub position_ms: u64,
    pub duration_ms: u64,
    pub volume: f32, // 0.0–1.0
    pub shuffle: bool,
    pub repeat: RepeatMode,
    pub device_id: Option<String>, // Spotify Connect device id reported by the hidden WebView
    /// Visual like-state for the current track. Backed by the real
    /// `PUT/DELETE /me/tracks` API in Phase 4; today it's an optimistic stub.
    pub liked: bool,
}

impl PlayerState {
    /// Comma-separated artist names for the current track ("" when idle).
    pub fn subtitle(&self) -> String {
        self.track
            .as_ref()
            .map(|t| {
                t.artists
                    .iter()
                    .map(|a| a.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default()
    }

    /// The artwork URL best suited for large surfaces (now-playing view):
    /// the biggest available image.
    pub fn large_art_url(&self) -> String {
        self.track
            .as_ref()
            .map(|t| {
                t.album
                    .images
                    .iter()
                    .max_by_key(|img| img.width.unwrap_or(0))
                    .map(|img| img.url.clone())
                    .unwrap_or_default()
            })
            .unwrap_or_default()
    }

    /// Append `track` to the play queue, de-duplicating by id first (re-adds to
    /// the tail). The SDK reports its own queue separately; this is the app's
    /// local view consumed by the Queue panel and `player::next()`.
    pub fn enqueue(&mut self, track: Track) {
        self.queue.retain(|t| t.id != track.id);
        self.queue.push(track);
    }

    /// Bulk enqueue with the same dedup-by-id semantics.
    pub fn enqueue_many(&mut self, tracks: impl IntoIterator<Item = Track>) {
        for t in tracks {
            self.enqueue(t);
        }
    }

    /// The next track the local queue would advance to (without consuming it).
    pub fn queue_next(&self) -> Option<&Track> {
        self.queue.first()
    }

    /// Consume the front of the local queue.
    pub fn pop_queue_head(&mut self) -> Option<Track> {
        if self.queue.is_empty() {
            None
        } else {
            Some(self.queue.remove(0))
        }
    }

    /// Enable/disable local shuffle. On the way in we snapshot the current id
    /// order so toggling off can restore it; on the way out we rebuild the
    /// queue to that order. Uses a seeded (not crypto) shuffle so the same
    /// starting order yields a stable shuffled sequence.
    pub fn set_shuffle(&mut self, on: bool) {
        if on {
            if self.queue_original.is_empty() {
                self.queue_original = self.queue.iter().map(|t| t.id.clone()).collect();
            }
            seeded_shuffle(&mut self.queue, shuffle_seed());
            self.shuffle = true;
        } else {
            self.shuffle = false;
            let snapshot = self.queue_original.clone();
            if !snapshot.is_empty() {
                self.restore_order(&snapshot);
            }
        }
    }

    fn restore_order(&mut self, original_ids: &[String]) {
        let mut pool: std::collections::HashMap<String, Track> =
            std::collections::HashMap::new();
        for t in self.queue.drain(..) {
            pool.insert(t.id.clone(), t);
        }
        let mut ordered = Vec::new();
        for id in original_ids {
            if let Some(t) = pool.remove(id) {
                ordered.push(t);
            }
        }
        for (_, t) in pool {
            ordered.push(t);
        }
        self.queue = ordered;
    }
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

/// Persisted user preferences (theme, volume, playback engine). Seeded from
/// `settings.json`; mutated via `ui::theme::set_theme` and the Settings page.
pub static SETTINGS: GlobalSignal<crate::settings::Settings> =
    Signal::global(crate::settings::Settings::load);

/// Query typed into the top-bar search field, handed off to the Search page
/// on Enter. The page consumes (takes) it on mount so back-navigation never
/// re-seeds stale text.
pub static SEARCH_SEED: GlobalSignal<String> = Signal::global(String::new);

/// Whether the now-playing right column is shown (≥1280 px viewports).
pub static SHOW_NOW_PLAYING: GlobalSignal<bool> = Signal::global(|| true);

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

/// Format a duration for display everywhere (`m:ss`), clamping nonsense.
/// `0` / unknown renders as `-:--` like Spotify does for live tracks.
pub fn format_duration(ms: u64) -> String {
    if ms == 0 {
        return "-:--".to_string();
    }
    let total_secs = ms / 1000;
    let mins = total_secs / 60;
    let secs = total_secs % 60;
    format!("{mins}:{secs:02}")
}

/// Mix entropy into a u64 -- used only to seed a local queue shuffle.
fn shuffle_seed() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_nanos() as u64).unwrap_or(0x9e3779b9)
}

/// In-place Fisher-Yates using a seeded PRNG. O(n), no allocations.
fn seeded_shuffle<T>(items: &mut [T], seed: u64) {
    let n = items.len();
    if n < 2 { return; }
    let mut state = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut rand = || { state ^= state >> 13; state ^= state << 7; state ^= state >> 17; state };
    for i in (1..n).rev() { let j = (rand() % (i as u64 + 1)) as usize; items.swap(i, j); }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_format_as_mss() {
        assert_eq!(format_duration(0), "-:--");
        assert_eq!(format_duration(1_000), "0:01");
        assert_eq!(format_duration(9_500), "0:09"); // sub-second truncated
        assert_eq!(format_duration(61_000), "1:01");
        assert_eq!(format_duration(267_111), "4:27");
        assert_eq!(format_duration(3_600_000), "60:00");
    }

    #[test]
    fn player_subtitle_joins_artists() {
        use crate::spotify::models::{ArtistRef, Track};
        let mut ps = PlayerState::default();
        assert_eq!(ps.subtitle(), "");

        let mk_artist = |name: &str| ArtistRef {
            id: name.to_string(),
            name: name.to_string(),
            uri: format!("spotify:artist:{name}"),
        };
        ps.track = Some(Track {
            id: "t".into(),
            name: "Mercy".into(),
            uri: "spotify:track:t".into(),
            duration_ms: 200_000,
            explicit: false,
            artists: vec![mk_artist("Kanye"), mk_artist("Chance")],
            album: Default::default(),
            preview_url: None,
            popularity: 0,
        });
        assert_eq!(ps.subtitle(), "Kanye, Chance");
    }

    #[test]
    fn large_art_prefers_widest_image() {
        use crate::spotify::models::{AlbumRef, SpotifyImage, Track};
        let mut ps = PlayerState::default();
        assert_eq!(ps.large_art_url(), "");

        ps.track = Some(Track {
            id: "t".into(),
            name: "x".into(),
            uri: "spotify:track:t".into(),
            duration_ms: 1,
            explicit: false,
            artists: vec![],
            album: AlbumRef {
                id: "a".into(),
                name: "album".into(),
                uri: "spotify:album:a".into(),
                images: vec![
                    SpotifyImage { url: "small".into(), width: Some(64), height: Some(64) },
                    SpotifyImage { url: "large".into(), width: Some(640), height: Some(640) },
                    SpotifyImage { url: "mid".into(), width: Some(300), height: Some(300) },
                ],
                album_type: None,
                release_date: None,
            },
            preview_url: None,
            popularity: 0,
        });
        assert_eq!(ps.large_art_url(), "large");
    }

    #[test]
    fn enqueue_dedups_by_id_and_keeps_tail_order() {
        let mut ps = PlayerState::default();
        ps.enqueue(mk_track("a"));
        ps.enqueue(mk_track("b"));
        ps.enqueue(mk_track("a"));
        assert_eq!(ids(&ps.queue), ["b", "a"]);
    }

    #[test]
    fn enqueue_many_dedups_and_preserves_input_order() {
        let mut ps = PlayerState::default();
        ps.enqueue_many(vec![mk_track("a"), mk_track("b"), mk_track("a")]);
        assert_eq!(ids(&ps.queue), ["b", "a"]);
    }

    #[test]
    fn pop_queue_head_consume_from_front() {
        let mut ps = PlayerState::default();
        ps.enqueue_many(vec![mk_track("a"), mk_track("b"), mk_track("c")]);
        assert_eq!(ps.pop_queue_head().unwrap().id, "a");
        assert_eq!(ps.pop_queue_head().unwrap().id, "b");
        assert_eq!(ps.queue_next().unwrap().id, "c");
    }

    #[test]
    fn shuffle_then_unshuffle_restores_original_order() {
        let mut ps = PlayerState::default();
        ps.enqueue_many(vec![mk_track("a"), mk_track("b"), mk_track("c"), mk_track("d")]);
        let original = ids(&ps.queue).to_vec();
        ps.set_shuffle(true);
        assert!(ps.shuffle);
        let mut shuffled = ids(&ps.queue);
        shuffled.sort();
        assert_eq!(shuffled, original);
        ps.set_shuffle(false);
        assert_eq!(ids(&ps.queue), original);
    }

    fn mk_track(id: &str) -> Track {
        Track {
            id: id.to_string(),
            name: id.to_string(),
            uri: format!("spotify:track:{id}"),
            duration_ms: 0,
            explicit: false,
            artists: vec![],
            album: Default::default(),
            preview_url: None,
            popularity: 0,
        }
    }

    fn ids(q: &[Track]) -> Vec<String> {
        q.iter().map(|t| t.id.clone()).collect()
    }
}