//! Spotify DX native core (Phase 0 of `docs/KOTLIN_MIGRATION.md`).
//!
//! Every service module lives here unchanged: authentication/session logic,
//! data fetchers and models, the filter engine, the streaming
//! resolver/providers/cache, audio decoding, the artwork cache,
//! settings/profile/token stores, the updater checker/stager, and the shared
//! state and error types. The binary (`src/main.rs`) keeps only the
//! renderer entry points plus the shared `bootstrap()` and is a thin shell
//! over this library.
//!
//! The `bridge` module (Android only) is the sole new native code: the
//! versioned C-ABI surface the Kotlin interface calls through JNI. The core
//! never depends on Kotlin except through registered listener callbacks.

// NOTE: `deny`, not `forbid`. The Android JNI bridge must export unmangled
// `Java_…` symbols, and current rustc classifies `#[no_mangle]` under
// `unsafe_code` — a crate-level `forbid` would reject every bridge export
// with no per-item override possible. `deny` keeps `unsafe {}` blocks a hard
// error everywhere while letting `bridge.rs` scope `#[allow(unsafe_code)]` to
// the export attributes only (the bridge still contains zero `unsafe` blocks).
#![deny(unsafe_code)]

pub mod adblock;
pub mod app;
pub mod app_error;
pub mod auth;
pub mod media;
pub mod player;
pub mod platform;
pub mod profile;
pub mod settings;
pub mod spotify;
pub mod state;
pub mod streaming;
pub mod ui;
pub mod updater;
pub mod util;

/// Versioned JNI bridge to the Kotlin Android interface (see §6 of
/// `docs/KOTLIN_MIGRATION.md`). Android only: it needs the `jni` crate, which
/// is an Android-target dependency. All other targets build the core without
/// it.
#[cfg(target_os = "android")]
pub mod bridge;
