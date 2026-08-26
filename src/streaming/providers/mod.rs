//! Provider implementations.

pub mod qobuz;
pub mod tidal;
pub mod youtube;

use crate::streaming::provider::Provider;

/// Build the ordered provider list. Providers are tried in order; the first
/// `Success` wins. On `NotFound`/`Error` the resolver falls through to the
/// next provider. On `Cooldown` the resolver either skips or waits briefly.
///
/// Order is informed by RESEARCH §2.3 / SYSTEM_DESIGN §6.7:
/// TIDAL (fastest, community instances) → Qobuz (ISRC match, high quality)
/// → YouTube (always available, lower quality).
pub fn build_provider_chain() -> Vec<Box<dyn Provider>> {
    vec![
        Box::new(tidal::TidalProvider::new()),
        Box::new(qobuz::QobuzProvider::new()),
        Box::new(youtube::YoutubeProvider::new()),
    ]
}
