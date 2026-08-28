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
            report_playback_err(&err);
        }
    });
}

/// Launch playback with metadata already in hand (the common case: the UI has
/// the full `Track` object the user clicked). Avoids a `/v1` metadata re-fetch.
pub fn launch_track(track: crate::spotify::models::Track) {
    dioxus::prelude::spawn(async move {
        if let Err(err) = play_track(&track).await {
            report_playback_err(&err);
        }
    });
}

fn report_playback_err(err: &AppError) {
    if matches!(err, AppError::PremiumRequired(_)) {
        crate::state::publish_error(AppError::PremiumRequired(
            "Playback requires Spotify Premium — your account can browse freely."
                .into(),
        ));
    }
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

/// Whether the open streaming engine (local audio sink) is the active backend.
/// Exposed for UI components that must behave differently on that path.
pub fn is_open_engine() -> bool {
    should_use_open_engine()
}

/// Start playing the given Spotify URI.
///
/// Routes to either the SDK (Premium) or the open streaming engine (free/forced)
/// based on `EnginePreference` in Settings. Resolves track metadata first.
pub async fn play_uri(uri: &str) -> Result<(), AppError> {
    let track_id = uri
        .strip_prefix("spotify:track:")
        .unwrap_or(uri);
    // Look up the track's metadata. Prefer the internal GraphQL API over `/v1`,
    // which is hard rate-limited on free/web tokens.
    let track = crate::spotify::api::get_track(track_id)
        .await
        .map_err(|e| AppError::Playback(format!("failed to fetch track metadata: {e}")))?;
    play_track(&track).await
}

/// Start playing a track whose metadata we already have (no network fetch).
pub async fn play_track(track: &crate::spotify::models::Track) -> Result<(), AppError> {
    if should_use_open_engine() {
        return open_play_track(track).await;
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
    crate::spotify::player_api::play(&device_id, &track.uri, None).await
}

/// Play a track via the open streaming engine.
///
/// Resolves the track through the provider chain and plays the result through
/// the audio sink. Reuses the metadata already present on `track`.
async fn open_play_track(track: &crate::spotify::models::Track) -> Result<(), AppError> {
    // Resolve through the open engine.
    let stream = crate::streaming::resolver::resolve(track)
        .await
        .map_err(|e| AppError::Playback(format!("resolver error: {e}")))?;
    match stream {
        Some(s) => {
            tracing::info!("open engine: playing {} via {}", track.name, s.provider);

            // Record the track in the UI state first so the bar renders.
            {
                let mut st = PLAYER_STATE.write();
                st.track = Some(track.clone());
                // Prefer the real track duration (from in-hand metadata) over
                // the sink's `total_duration()`, which is unreliable for muxed
                // MP4 (symphonia reports a short metadata duration).
                st.duration_ms = track.duration_ms;
                st.position_ms = 0;
                st.is_playing = true;
                st.device_id = None;
            }

            // (Lazily) start the audio sink and hand it the stream URL.
            let sink = crate::media::sink::global_sink(current_volume());
            let (tx, state) = sink;
            let current_vol = current_volume();
            let _ = tx.send(crate::media::sink::SinkCommand::Play {
                url: s.url.clone(),
                format: s.format,
            });
            // Re-apply the current volume (the sink thread seeds from the
            // volume captured on its first spawn, which may be stale now).
            let _ = tx.send(crate::media::sink::SinkCommand::Volume(current_vol));

            // Keep the UI clock in sync with the sink thread. Runs until the
            // next track's open_play_track spawns a fresh poller (no break on
            // pause so resume keeps syncing).
            dioxus::prelude::spawn(async move {
                let mut ticker = tokio::time::interval(std::time::Duration::from_millis(250));
                loop {
                    ticker.tick().await;
                    let mut st = PLAYER_STATE.write();
                    st.position_ms = state
                        .position_ms
                        .load(std::sync::atomic::Ordering::Relaxed)
                        .min(st.duration_ms.max(1));
                    // Only fall back to the sink's metadata duration when we
                    // don't already have the real track duration (symphonia's
                    // `total_duration()` is unreliable for muxed MP4).
                    if st.duration_ms == 0 {
                        st.duration_ms = state
                            .duration_ms
                            .load(std::sync::atomic::Ordering::Relaxed);
                    }
                    // Mirror the sink play state once it has actually begun
                    // playing (`duration_ms` is set right before playback starts),
                    // avoiding a brief false "paused" during buffering.
                    if state.duration_ms.load(std::sync::atomic::Ordering::Relaxed) != 0 {
                        st.is_playing = state
                            .is_playing
                            .load(std::sync::atomic::Ordering::Relaxed);
                    }
                }
            });

            Ok(())
        }
        None => Err(AppError::Playback(
            "could not find this track on any provider".into(),
        )),
    }
}

/// Current desired volume (0.0–1.0), used to seed the sink and adjust it.
fn current_volume() -> f32 {
    use dioxus::prelude::ReadableExt;
    PLAYER_STATE.peek().volume.clamp(0.0, 1.0)
}

/// Seed `PLAYER_STATE.volume` from the persisted settings exactly once, so the
/// audio sink isn't started at the derive-default 0.0 (which would be silent).
pub fn seed_volume_from_settings() {
    use dioxus::prelude::ReadableExt;
    let persisted = crate::state::SETTINGS.peek().volume.clamp(0.0, 1.0);
    // Only seed if the player volume hasn't been explicitly set yet.
    if PLAYER_STATE.peek().volume == 0.0 {
        PLAYER_STATE.write().volume = persisted;
    }
}

pub async fn play() -> Result<(), AppError> {
    if should_use_open_engine() {
        let (tx, _state) = crate::media::sink::global_sink(current_volume());
        let _ = tx.send(crate::media::sink::SinkCommand::Resume);
        PLAYER_STATE.write().is_playing = true;
        return Ok(());
    }
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
    if should_use_open_engine() {
        let (tx, _state) = crate::media::sink::global_sink(current_volume());
        let _ = tx.send(crate::media::sink::SinkCommand::Pause);
        PLAYER_STATE.write().is_playing = false;
        return Ok(());
    }
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
        return play_track(&track).await;
    }
    if should_use_open_engine() {
        // Nothing queued; restart current track.
        if let Some(current) = PLAYER_STATE.peek().track.clone() {
            return play_track(&current).await;
        }
        return Err(AppError::Playback("no current track".into()));
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
    if should_use_open_engine() {
        // Restart the current track from the beginning.
        if let Some(current) = PLAYER_STATE.peek().track.clone() {
            return play_track(&current).await;
        }
        return Err(AppError::Playback("no current track".into()));
    }
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
    if should_use_open_engine() {
        let (tx, _state) = crate::media::sink::global_sink(current_volume());
        let _ = tx.send(crate::media::sink::SinkCommand::Seek(ms));
        PLAYER_STATE.write().position_ms = ms;
        return Ok(());
    }
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
    if should_use_open_engine() {
        let (tx, _state) = crate::media::sink::global_sink(v);
        let _ = tx.send(crate::media::sink::SinkCommand::Volume(v));
        PLAYER_STATE.write().volume = v;
        return Ok(());
    }
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