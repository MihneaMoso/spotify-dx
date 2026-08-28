//! Provider implementations.

pub mod qobuz;
pub mod tidal;
pub mod youtube;

use crate::streaming::provider::Provider;

/// Build the ordered provider list. Providers are tried in order; the first
/// `Success` wins. On `NotFound`/`Error` the resolver falls through to the
/// next provider. On `Cooldown` the resolver either skips or waits briefly.
///
/// TIDAL and Qobuz are compiled in but report `is_available() == false`: Odesli
/// (song.link), their only Spotify→platform ID mapper, was sunset (401,
/// key-gated), so they can't resolve. They're kept for a future working mapper
/// but skipped by the resolver. YouTube is the active, self-contained provider
/// (InnerTube search + player; no external ID mapping).
pub fn build_provider_chain() -> Vec<Box<dyn Provider>> {
    vec![
        Box::new(tidal::TidalProvider::new()),
        Box::new(qobuz::QobuzProvider::new()),
        Box::new(youtube::YoutubeProvider::new()),
    ]
}
