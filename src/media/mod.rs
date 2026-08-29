//! Media pipeline: local decoding/playback support for the open engine
//! (`SYSTEM_DESIGN.md` §6.7). Artwork caching joins here in Phase 4.
//!
//! `audio` (symphonia decode helpers) and `sink` (rodio/cpal output thread) are
//! native-only; wasm swaps the sink for a browser `HtmlAudioElement`-driven
//! implementation (`sink_wasm`) with the same public API.

#[cfg(not(target_arch = "wasm32"))]
pub mod audio;
pub mod images;
#[cfg(not(target_arch = "wasm32"))]
pub mod sink;

#[cfg(target_arch = "wasm32")]
pub mod sink_wasm;
#[cfg(target_arch = "wasm32")]
pub use sink_wasm as sink;
