//! Provider implementations.

pub mod audius;
pub mod piped;
pub mod qobuz;
pub mod saavn;
pub mod soundcloud;
pub mod tidal;
pub mod youtube;

use crate::streaming::provider::Provider;

/// Build the ordered provider list. Providers are tried in order; the first
/// `Success` wins. On `NotFound`/`Error` the resolver falls through to the
/// next provider. On `Cooldown` the resolver either skips or waits briefly.
///
/// TIDAL stays parked (`is_available() == false`): its mapper (Odesli) is
/// sunset and a direct revival needs real credentials to verify against.
/// Qobuz revives when the user supplies credentials (Phase F); otherwise it
/// is skipped at zero cost like TIDAL. Then YouTube (self-contained),
/// Piped, Saavn, Audius, SoundCloud.
pub fn build_provider_chain() -> Vec<Box<dyn Provider>> {
    vec![
        Box::new(tidal::TidalProvider::new()),
        Box::new(qobuz::QobuzProvider::new()),
        Box::new(youtube::YoutubeProvider::new()),
        Box::new(piped::PipedProvider::new()),
        Box::new(saavn::SaavnProvider::new()),
        Box::new(audius::AudiusProvider::new()),
        Box::new(soundcloud::SoundcloudProvider::new()),
    ]
}

/// Cache probe order: MUST name every chain member, else that provider's
/// cache entries become write-only (resolver probes by name). The unit test
/// below enforces the match.
pub const CACHE_PROBE_ORDER: &[&str] = &[
    "tidal",
    "qobuz",
    "youtube",
    "piped",
    "saavn",
    "audius",
    "soundcloud",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_probe_order_matches_chain() {
        let chain_names: Vec<&str> =
            build_provider_chain().iter().map(|p| p.name()).collect();
        assert_eq!(chain_names, CACHE_PROBE_ORDER);
    }
}
