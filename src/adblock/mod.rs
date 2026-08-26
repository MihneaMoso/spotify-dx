pub mod adguard_api;
pub mod engine;

use crate::app_error::AppError;
use crate::state::AdblockStats;

/// Seed the filter: load the bundled snapshot, compile or restore the engine,
/// then refresh in the background.
pub async fn init() -> anyhow::Result<()> {
    adguard_api::init().await
}

/// Decide whether a URL should be dropped by the ad filter.  Delegates to the
/// Brave-style `adblock` engine which handles hosts syntax, AdGuard syntax,
/// and exception rules natively.
pub fn should_block(url: &str) -> bool {
    engine::should_block_url(url)
}

/// Pull the hostname out of an arbitrary URL string.
pub fn extract_host(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    parsed.host_str().map(|host| host.to_ascii_lowercase())
}

/// Current number of compiled rules held in the engine.
pub fn block_count() -> usize {
    engine::block_count()
}

/// Whether the engine has any rules loaded (used by the UI badge).
pub fn is_ready() -> bool {
    engine::is_ready()
}

/// Record one blocked request. Thread-safe; callable from the HTTP layer.
pub fn record_drop() {
    adguard_api::record_drop();
}

/// Snapshot of the filter state for the UI to sync into `ADBLOCK_STATS`.
pub fn stats_snapshot() -> AdblockStats {
    adguard_api::snapshot()
}

/// Resolve a hostname through DNS-over-HTTPS (Cloudflare).
pub async fn dns_resolve(host: &str) -> Result<Vec<std::net::IpAddr>, AppError> {
    engine::dns_resolve(host)
        .await
        .map_err(|err| AppError::Other(anyhow::anyhow!(err.to_string())))
}

/// Inject cosmetic CSS into a WebView page to hide Spotify upsell elements.
/// Gated behind `hide_upsell` in Settings (disabled by default).
pub fn cosmetic_css_injection() -> &'static str {
    use crate::settings::Settings;
    let settings = Settings::load();
    if settings.hide_upsell {
        cosmetic::HIDE_UPSELL_CSS
    } else {
        ""
    }
}

/// Cosmetic CSS rules targeting Spotify upsell / ad elements (SpotiCap tier 2).
/// See `docs/RESEARCH.md` §3.3.
pub mod cosmetic {
    /// CSS injected into the login/session WebView to hide premium upgrade
    /// buttons, HPTO banners, and sponsored items.
    pub const HIDE_UPSELL_CSS: &str = r#"
        [class*="UpgradeButton"],
        a[href*="/premium/"],
        .main-leaderboardComponent-container,
        div[data-testid*="hpto"],
        div[data-testid*="ad-"],
        [data-testid="sponsored-item"],
        iframe[src*="doubleclick"],
        iframe[src*="googlesyndication"],
        [class*="PremiumBadge"],
        [data-testid="premium-upsell"] {
            display: none !important;
        }
    "#;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_known_ad_domain_blocked() {
        let engine = engine::build_engine("0.0.0.0 ads.doubleclick.net\n");
        let req = adblock::request::Request::new(
            "https://ads.doubleclick.net/some/path?q=1",
            "open.spotify.com",
            "xhr",
            "GET",
        )
        .unwrap();
        assert!(engine.check_network_request(&req).should_block());
    }

    #[test]
    fn test_spotify_domains_never_blocked() {
        let engine = engine::build_engine("0.0.0.0 ads.spotify.com\n");
        let urls = [
            "https://ads.spotify.com/some/path?q=1",
            "https://audio-ak.spotifycdn.com/track.mp4",
        ];
        for url in &urls {
            let req = adblock::request::Request::new(url, "open.spotify.com", "xhr", "GET").unwrap();
            assert!(
                !engine.check_network_request(&req).should_block(),
                "must not block {url}"
            );
        }
    }

    #[test]
    fn test_spotify_api_not_blocked() {
        let engine = engine::build_engine("0.0.0.0 ads.spotify.com\n||doubleclick.net^\n");
        let urls = [
            "https://api.spotify.com/v1/me",
            "https://accounts.spotify.com/authorize",
            "https://i.scdn.co/image/abc",
            "https://audio-ak.spotifycdn.com/track.mp4",
        ];
        for url in &urls {
            let req = adblock::request::Request::new(url, "open.spotify.com", "xhr", "GET").unwrap();
            assert!(
                !engine.check_network_request(&req).should_block(),
                "must not block {url}"
            );
        }
    }

    #[test]
    fn test_extract_host_normalizes() {
        assert_eq!(extract_host("https://Example.COM/path"), Some("example.com".into()));
        assert_eq!(extract_host("not a url"), None);
    }

    #[test]
    fn test_cosmetic_css_is_nonempty() {
        assert!(!cosmetic::HIDE_UPSELL_CSS.is_empty());
        assert!(cosmetic::HIDE_UPSELL_CSS.contains("UpgradeButton"));
    }
}
