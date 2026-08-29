//! Wasm audio sink: browser `HtmlAudioElement` output.
//!
//! Mirrors the native `media::sink` public API (`SinkCommand`, `SinkState`,
//! `global_sink`, `sink_state`, `spawn_sink`) so `player/` is platform-agnostic.
//! The browser element owns decoding, seeking, pausing and volume; this module
//! bridges commands onto it and mirrors position/duration/playing into the
//! shared `SinkState` (which the UI's single position ticker reads).
//!
//! Runs on the single wasm thread, so commands are drained from a `std::mpsc`
//! channel by a small async driver task rather than a background thread.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use wasm_bindgen::JsCast;
use web_sys::HtmlAudioElement;

/// Commands sent from the UI to the audio sink.
#[derive(Debug)]
pub enum SinkCommand {
    /// Load and play a new track from a URL.
    Play {
        url: String,
        format: crate::streaming::provider::AudioFormat,
    },
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

/// Global sink handle: command sender + shared state. Lazily spawned on first
/// open-engine use; lives for the whole page.
type SinkHandle = (std::sync::mpsc::Sender<SinkCommand>, Arc<SinkState>);

static GLOBAL: OnceLock<SinkHandle> = OnceLock::new();

/// Get (and on first call, spawn) the page-wide audio sink.
pub fn global_sink(initial_volume: f32) -> &'static SinkHandle {
    GLOBAL.get_or_init(|| spawn_sink(initial_volume))
}

/// Non-spawning access to the sink's shared state, if started at all.
pub fn sink_state() -> Option<&'static SinkState> {
    GLOBAL.get().map(|(_, state)| &**state)
}

/// Shared state between the UI and the audio driver.
pub struct SinkState {
    pub position_ms: AtomicU64,
    pub duration_ms: AtomicU64,
    pub is_playing: AtomicBool,
    pub is_buffering: AtomicBool,
    /// Notified each time a fresh `position_ms` is published while playing.
    pub position_changed: tokio::sync::Notify,
}

impl Default for SinkState {
    fn default() -> Self {
        Self {
            position_ms: AtomicU64::new(0),
            duration_ms: AtomicU64::new(0),
            is_playing: AtomicBool::new(false),
            is_buffering: AtomicBool::new(true),
            position_changed: tokio::sync::Notify::new(),
        }
    }
}

fn create_element() -> Option<HtmlAudioElement> {
    let window = web_sys::window()?;
    let doc = window.document()?;
    let el: HtmlAudioElement = doc.create_element("audio").ok()?.dyn_into().ok()?;
    el.set_preload("auto");
    Some(el)
}

fn current_ms(el: &HtmlAudioElement) -> u64 {
    let s = el.current_time();
    if s.is_finite() && s > 0.0 {
        (s * 1000.0) as u64
    } else {
        0
    }
}

/// Native-identical `spawn_sink` API: returns a command sender + shared state.
/// The returned sender drives a lazily-created element owned by an async driver
/// task running on the single wasm thread.
pub fn spawn_sink(initial_volume: f32) -> (
    std::sync::mpsc::Sender<SinkCommand>,
    Arc<SinkState>,
) {
    let (tx, rx) = std::sync::mpsc::channel();
    let state = Arc::new(SinkState::default());
    let driver_state = state.clone();
    let initial_volume = initial_volume.clamp(0.0, 1.0);

    wasm_bindgen_futures::spawn_local(async move {
        let Some(el) = create_element() else {
            driver_state.is_buffering.store(false, Ordering::Relaxed);
            return;
        };
        el.set_volume(initial_volume as f64);

        let mut shutting_down = false;
        while !shutting_down {
            // Publish position while "playing".
            if driver_state.is_playing.load(Ordering::Relaxed) {
                driver_state.position_ms.store(current_ms(&el), Ordering::Relaxed);
                driver_state.position_changed.notify_waiters();
            }
            let duration_s = el.duration();
            if duration_s.is_finite() && duration_s > 0.0 {
                driver_state.duration_ms.store((duration_s * 1000.0) as u64, Ordering::Relaxed);
            }

            // Drain all queued commands.
            while let Ok(cmd) = rx.try_recv() {
                match cmd {
                    SinkCommand::Play { url, format } => {
                        driver_state.is_buffering.store(true, Ordering::Relaxed);
                        driver_state.is_playing.store(true, Ordering::Relaxed);
                        driver_state.position_ms.store(0, Ordering::Relaxed);
                        el.set_src(&url);
                        let _ = el.play();
                        driver_state.duration_ms.store(0, Ordering::Relaxed);
                        tracing::info!("wasm sink: playing {url} ({format:?})");
                        driver_state.is_buffering.store(false, Ordering::Relaxed);
                    }
                    SinkCommand::Pause => {
                        let _ = el.pause();
                        driver_state.is_playing.store(false, Ordering::Relaxed);
                    }
                    SinkCommand::Resume => {
                        let _ = el.play();
                        driver_state.is_playing.store(true, Ordering::Relaxed);
                    }
                    SinkCommand::Seek(ms) => {
                        el.set_current_time(ms as f64 / 1000.0);
                        driver_state.position_ms.store(ms, Ordering::Relaxed);
                        driver_state.position_changed.notify_waiters();
                    }
                    SinkCommand::Volume(v) => {
                        el.set_volume(v.clamp(0.0, 1.0) as f64);
                    }
                    SinkCommand::Shutdown => {
                        let _ = el.pause();
                        el.set_src("");
                        shutting_down = true;
                    }
                }
            }

            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    });

    (tx, state)
}
