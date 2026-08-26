//! Audio sink: rodio output + progressive fetch.
//!
//! The open engine's audio pipeline:
//! 1. Fetch audio data via HTTP.
//! 2. Decode with rodio's built-in decoder (symphonia-backed).
//! 3. Output via rodio's `Player` + `Mixer`.
//!
//! This module runs on a dedicated thread (rodio's `Player` is `!Send`).

use std::io::Cursor;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Commands sent from the UI thread to the audio sink thread.
pub enum SinkCommand {
    /// Load and play a new track from a URL.
    Play { url: String, format: String },
    /// Pause playback.
    Pause,
    /// Resume playback.
    Resume,
    /// Seek to a position (milliseconds).
    Seek(u64),
    /// Set volume (0.0–1.0).
    Volume(f32),
    /// Stop everything and shut down.
    Shutdown,
}

/// Shared state between the UI and the sink thread.
pub struct SinkState {
    pub position_ms: AtomicU64,
    pub duration_ms: AtomicU64,
    pub is_playing: AtomicBool,
    pub is_buffering: AtomicBool,
}

impl Default for SinkState {
    fn default() -> Self {
        Self {
            position_ms: AtomicU64::new(0),
            duration_ms: AtomicU64::new(0),
            is_playing: AtomicBool::new(false),
            is_buffering: AtomicBool::new(true),
        }
    }
}

/// Start the audio sink on a background thread. Returns a command sender and
/// shared state.
pub fn spawn_sink(initial_volume: f32) -> (
    std::sync::mpsc::Sender<SinkCommand>,
    Arc<SinkState>,
) {
    let (tx, rx) = std::sync::mpsc::channel();
    let state = Arc::new(SinkState::default());
    let state_clone = state.clone();

    std::thread::Builder::new()
        .name("audio-sink".into())
        .spawn(move || {
            sink_loop(rx, state_clone, initial_volume);
        })
        .expect("failed to spawn audio sink thread");

    (tx, state)
}

/// The main loop running on the sink thread.
fn sink_loop(rx: std::sync::mpsc::Receiver<SinkCommand>, state: Arc<SinkState>, initial_vol: f32) {
    let mixer_sink = match rodio::stream::DeviceSinkBuilder::open_default_sink() {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("audio sink: failed to open default sink: {e}");
            return;
        }
    };
    let mixer = mixer_sink.mixer();

    let mut current_player: Option<rodio::Player> = None;

    while let Ok(cmd) = rx.recv() {
        match cmd {
            SinkCommand::Play { url, format } => {
                state.is_buffering.store(true, Ordering::Relaxed);
                state.is_playing.store(false, Ordering::Relaxed);

                // Stop any current playback.
                if let Some(p) = current_player.take() {
                    p.stop();
                }

                // Fetch the audio data.
                let data = match fetch_audio_bytes(&url) {
                    Ok(d) => d,
                    Err(e) => {
                        tracing::error!("audio sink: fetch failed: {e}");
                        state.is_buffering.store(false, Ordering::Relaxed);
                        continue;
                    }
                };

                let cursor = Cursor::new(data);
                match rodio::play(mixer, cursor) {
                    Ok(player) => {
                        player.set_volume(initial_vol);
                        state.is_buffering.store(false, Ordering::Relaxed);
                        state.is_playing.store(true, Ordering::Relaxed);
                        current_player = Some(player);
                        tracing::info!("audio sink: playing track ({format})");
                    }
                    Err(e) => {
                        tracing::error!("audio sink: decode/play failed: {e}");
                        state.is_buffering.store(false, Ordering::Relaxed);
                    }
                }
            }
            SinkCommand::Pause => {
                if let Some(ref p) = current_player {
                    p.pause();
                    state.is_playing.store(false, Ordering::Relaxed);
                }
            }
            SinkCommand::Resume => {
                if let Some(ref p) = current_player {
                    p.play();
                    state.is_playing.store(true, Ordering::Relaxed);
                }
            }
            SinkCommand::Seek(ms) => {
                if let Some(ref p) = current_player {
                    let _ = p.try_seek(Duration::from_millis(ms));
                    state.position_ms.store(ms, Ordering::Relaxed);
                }
            }
            SinkCommand::Volume(v) => {
                if let Some(ref p) = current_player {
                    p.set_volume(v);
                }
            }
            SinkCommand::Shutdown => {
                if let Some(p) = current_player.take() {
                    p.stop();
                }
                break;
            }
        }
    }
    tracing::info!("audio sink: thread exiting");
}

/// Fetch audio bytes from a URL. Uses the ad-blocked client.
fn fetch_audio_bytes(url: &str) -> Result<Vec<u8>, String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("tokio runtime: {e}"))?;
    rt.block_on(async {
        let resp = crate::spotify::client::filtered_get(url).await
            .map_err(|e| format!("ad-block filter: {e}"))?;
        let bytes = resp.bytes().await
            .map_err(|e| format!("download: {e}"))?;
        Ok(bytes.to_vec())
    })
}
