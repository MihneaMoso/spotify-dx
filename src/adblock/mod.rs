pub mod adguard_api;
pub mod dns_filter;

use crate::app_error::AppError;
use crate::state::AdblockStats;

/// Seed the filter: load the bundled snapshot, refresh in the background and
/// report what the graph looks like.
pub async fn init() -> anyhow::Result<()> {
    adguard_api::init().await
}

/// Decide whether a URL should be dropped by the ad filter. Extracts the host
/// and runs the O(k) trie lookup against the reversed-label index.
pub fn should_block(url: &str) -> bool {
    let Some(host) = extract_host(url) else {
        return false;
    };
    dns_filter::is_blocked(&host)
}

/// Pull the hostname out of an arbitrary URL string.
pub fn extract_host(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    parsed.host_str().map(|host| host.to_ascii_lowercase())
}

/// Current number of rules held in the tree (for the status panel).
pub fn block_count() -> usize {
    dns_filter::block_count()
}

/// Whether the filter has any rules loaded yet (used by the UI badge).
pub fn is_ready() -> bool {
    dns_filter::block_count() > 0
}

/// Record one blocked request. Thread-safe; callable from the HTTP layer.
pub fn record_drop() {
    adguard_api::record_drop();
}

/// Snapshot of the filter state for the UI to sync into `ADBLOCK_STATS`.
pub fn stats_snapshot() -> AdblockStats {
    adguard_api::snapshot()
}

/// Parse a blocklist line into a domain (also used by the merging code and tests).
pub fn parse_host_line(line: &str) -> Option<String> {
    dns_filter::parse_host(line)
}

/// Insert a single domain at runtime (used by tests and live-updates).
pub fn insert_blocked_domain(domain: &str) {
    dns_filter::insert_blocked_domain(domain);
}

/// Bulk-insert hosts-format content; returns the number of new entries.
pub fn load_hosts_content(content: &str) -> usize {
    dns_filter::load_hosts_content(content)
}

/// Resolve a hostname through the system resolver, falling back to DoH.
pub async fn resolve(host: &str) -> Result<Vec<std::net::IpAddr>, AppError> {
    dns_filter::resolve(host)
        .await
        .map_err(|err| AppError::Other(anyhow::anyhow!(err.to_string())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_known_ad_domain_blocked() {
        insert_blocked_domain("ads.doubleclick.net");
        assert!(should_block("https://ads.doubleclick.net/some/path?q=1"));
        assert!(should_block("https://banner.ads.doubleclick.net/track.gif"));
    }

    #[test]
    fn test_spotify_domains_never_blocked() {
        // Spotify's own domains are always let through: the web-session login,
        // token endpoint, API and audio streams all live there.
        insert_blocked_domain("ads.spotify.com");
        assert!(!should_block("https://ads.spotify.com/some/path?q=1"));
        assert!(!should_block("https://audio-ak.spotifycdn.com/track.mp4"));
    }

    #[test]
    fn test_spotify_api_not_blocked() {
        insert_blocked_domain("ads.spotify.com");
        assert!(!should_block("https://api.spotify.com/v1/me"));
        assert!(!should_block("https://accounts.spotify.com/authorize"));
        assert!(!should_block("https://i.scdn.co/image/abc"));
        assert!(!should_block("https://audio-ak.spotifycdn.com/track.mp4"));
    }

    #[test]
    fn test_trie_lookup_perf() {
        // Insert a realistic corpus…
        for i in 0..5_000 {
            insert_blocked_domain(&format!("tracker{i}.example{i}.com"));
        }
        // …then measure a million lookups.
        let start = std::time::Instant::now();
        let mut hits = 0u32;
        for i in 0..1_000_000 {
            let needle = format!("sub.tracker{}.example{}.com", i % 5_000, i % 5_000);
            if dns_filter::is_blocked(&needle) {
                hits += 1;
            }
        }
        let elapsed = start.elapsed();
        assert_eq!(hits, 1_000_000, "all generated needles must hit");
        assert!(
            elapsed.as_secs_f64() < 1.0,
            "1M lookups took {:?}, expected < 1s",
            elapsed
        );
    }

    #[test]
    fn test_hosts_file_parsing() {
        let sample = "\
# AdGuard DNS
! comment block
||ads.example.com^
||analytics.example.net^$third-party
0.0.0.0 track.example.org
127.0.0.1 pixel.example.io
api.spotify.com
0.0.0.0 0.0.0.0
";

        let count = load_hosts_content(sample);
        // ||ads.example.com^, ||analytics…^, track.example.org, pixel.example.io
        assert_eq!(count, 4);
        assert!(should_block("https://ads.example.com/x"));
        assert!(should_block("https://pixel.example.io/1"));
        assert!(!should_block("https://api.spotify.com/v1"));
        // The hosts-form `0.0.0.0 0.0.0.0` must be ignored entirely.
        assert_eq!(parse_host_line("0.0.0.0 0.0.0.0"), None);
    }
}