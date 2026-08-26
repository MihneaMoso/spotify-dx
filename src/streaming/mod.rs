//! Open multi-source streaming engine (`SYSTEM_DESIGN.md` §6.7).
//!
//! Resolves a Spotify track ID into a playable audio URL by:
//! 1. Mapping the Spotify ID → provider IDs via Odesli (song.link).
//! 2. Trying providers in priority order, each returning an explicit state.
//! 3. Caching resolved URLs (memory + disk, short TTL).
//!
//! The audio decode/sink layer lives in `media::sink`.

pub mod cache;
pub mod odesli;
pub mod provider;
pub mod providers;
pub mod resolver;
