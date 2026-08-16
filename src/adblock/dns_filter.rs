use once_cell::sync::Lazy;
use parking_lot::RwLock;
use radix_trie::Trie;

/// Hostnames that must never be blocked regardless of the blocklist content.
pub const ALWAYS_ALLOW: &[&str] = &[
    "api.spotify.com",
    "spclient.wg.spotify.com",
    "apresolve.spotify.com",
    "open.spotify.com",
    "accounts.spotify.com",
    "i.scdn.co",
    "sdk.scdn.co",
];

/// Returns `true` when `host` (or one of its parent domains) is on the blocklist.
pub fn is_blocked(host: &str) -> bool {
    let host = normalize_host(host);
    if host.is_empty() {
        return false;
    }
    if is_whitelisted(&host) {
        return false;
    }
    let key = reverse_domain(&host);
    FILTER.read().trie.get_ancestor_value(&key).is_some()
}

/// Number of entries currently held in the block tree.
pub fn block_count() -> usize {
    FILTER.read().trie_len
}

/// Insert a single domain (and therefore all of its subdomains) into the trie.
/// The trie `insert` is idempotent, so calling this twice is harmless.
pub fn insert_blocked_domain(domain: &str) {
    let Some(host) = parse_host(domain) else {
        return;
    };
    if is_whitelisted(&host) {
        return;
    }
    let key = reverse_domain(&host);
    let mut filter = FILTER.write();
    let existing = filter.trie.get(&key).is_some();
    if !existing {
        filter.trie.insert(key, ());
        filter.trie_len += 1;
    }
}

/// Extract a clean hostname from a single blocklist line.
///
/// Supports the three formats used by AdGuard / hosts lists:
///   * `||example.com^`        (AdGuard syntax)
///   * `0.0.0.0 example.com`   (hosts syntax)
///   * `example.com`           (bare domain)
pub fn parse_host(line: &str) -> Option<String> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
        return None;
    }
    // AdGuard domain rule, e.g. `||ads.example.com^` or `||ads.example.com$third-party`.
    if let Some(rest) = line.strip_prefix("||") {
        let token = rest.split(['^', '$']).next()?;
        return Some(normalize_host(token));
    }
    let mut parts = line.split_whitespace();
    let first = parts.next()?;
    // hosts syntax: an IP address first, the hostname second.
    if first.parse::<std::net::IpAddr>().is_ok() {
        let host = parts.next()?;
        if host.parse::<std::net::IpAddr>().is_ok() {
            // `0.0.0.0 0.0.0.0` type entries carry no hostname.
            return None;
        }
        if host.split('.').count() < 2 {
            return None; // `localhost`, `broadcasthost`, …
        }
        return Some(normalize_host(host));
    }
    // bare-domain syntax, e.g. `ads.example.com`
    if !first.contains('.') || first.contains(':') || first.split('.').count() < 2 {
        return None;
    }
    Some(normalize_host(first))
}

/// Bulk-insert every "interesting" host extracted from `content`, deduplicating
/// in memory before touching the trie. Returns the number of new entries.
pub fn load_hosts_content(content: &str) -> usize {
    let mut to_insert: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for line in content.lines() {
        if let Some(host) = parse_host(line) {
            if seen.insert(host.clone()) {
                to_insert.push(host);
            }
        }
    }
    // Respect the always-allow list before anything hits the index.
    to_insert.retain(|host| !is_whitelisted(host));
    let mut added = 0usize;
    {
        let mut filter = FILTER.write();
        for host in to_insert {
            let key = reverse_domain(&host);
            if filter.trie.get(&key).is_none() {
                filter.trie.insert(key, ());
                filter.trie_len += 1;
                added += 1;
            }
        }
    }
    added
}

/// A pure in-process resolver. Resolves via DNS-over-HTTPS against Cloudflare
/// (`1.1.1.1/dns-query`) so it works even when the system resolver is a hostile
/// ad-blocking DNS.
pub async fn resolve(host: &str) -> Result<Vec<std::net::IpAddr>, anyhow::Error> {
    use hickory_resolver::config::{ResolverConfig, ResolverOpts};
    use hickory_resolver::TokioAsyncResolver;

    let resolver =
        TokioAsyncResolver::tokio(ResolverConfig::cloudflare_https(), ResolverOpts::default());
    let resp = resolver.lookup_ip(host).await?;
    let ips: Vec<_> = resp.iter().collect();
    if ips.is_empty() {
        anyhow::bail!("no A/AAAA records for {host}");
    }
    Ok(ips)
}

// ── internals ────────────────────────────────────────────────────────────────

struct DnsFilter {
    trie: Trie<String, ()>,
    trie_len: usize,
}

impl Default for DnsFilter {
    fn default() -> Self {
        Self {
            trie: Trie::new(),
            trie_len: 0,
        }
    }
}

/// The single block index shared by the whole process. `parking_lot::RwLock` is
/// used because reads are frequent (every outgoing request) and there is no
/// contention with the UI thread's Dioxus signals.
static FILTER: Lazy<RwLock<DnsFilter>> = Lazy::new(|| RwLock::new(DnsFilter::default()));

/// Strip scheme-no-carriers, trailing dots, capitalization and empty labels.
fn normalize_host(host: &str) -> String {
    let host = host.trim().trim_matches('.').to_ascii_lowercase();
    host
}

/// Reverse the labels of a hostname so a domain and all of its subdomains end up
/// along a common prefix in the trie, e.g.
///   `ads.spotify.com` → `com.spotify.ads`
///   `foo.ads.spotify.com` → `com.spotify.ads.foo`
/// An ancestor lookup for `com.spotify.ads.foo` finds `com.spotify.ads`.
fn reverse_domain(host: &str) -> String {
    host.split('.')
        .filter(|label| !label.is_empty())
        .rev()
        .collect::<Vec<_>>()
        .join(".")
}

/// The whitelist is intentionally not granular: any host under one of these
/// names is let through. `audio*.spotifycdn.com` covers the stream hosts like
/// `audio-ak.spotifycdn.com` or `audio-fa.spotifycdn.com`.
fn is_whitelisted(host: &str) -> bool {
    if ALWAYS_ALLOW.contains(&host) {
        return true;
    }
    let host = host.trim_end_matches('.');
    if host.ends_with(".spotifycdn.com") && host.starts_with("audio.") {
        return true;
    }
    false
}

#[cfg(test)]
mod dns_tests {
    use super::*;

    #[test]
    fn reverse_domain_orders_labels() {
        assert_eq!(reverse_domain("ads.spotify.com"), "com.spotify.ads");
    }

    #[test]
    fn hosts_line_parse() {
        assert_eq!(
            parse_host("0.0.0.0 tracker.example.com"),
            Some("tracker.example.com".to_string())
        );
        assert_eq!(
            parse_host("127.0.0.1 ad.example.net"),
            Some("ad.example.net".to_string())
        );
        assert_eq!(parse_host("||ads.example.org^"), Some("ads.example.org".to_string()));
        assert_eq!(
            parse_host("||ads.example.io^$third-party"),
            Some("ads.example.io".to_string())
        );
        assert_eq!(parse_host("# comment"), None);
        assert_eq!(parse_host(""), None);
    }
}