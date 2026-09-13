//! Shared provider plumbing: query encoding and HTTP client construction.
//! New providers use these directly; existing providers migrate
//! opportunistically. Bodies are behavior-preserving — `urlencode` is a
//! byte-identical move of the four copies that lived in
//! audius/piped/saavn/soundcloud.

/// Percent-encode a query component (`+` for space, RFC 3986 unreserved
/// set left bare). Matches all four former per-provider copies exactly.
pub fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else if b == b' ' {
            out.push('+');
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// House HTTP client: caller UA + bounded timeout (timeouts are cfg'd out
/// on wasm, where the browser owns the fetch — same pattern as the
/// per-provider builders this replaces for new code).
pub fn http_client(user_agent: &str, timeout_secs: u64) -> reqwest::Client {
    let builder = reqwest::Client::builder().user_agent(user_agent);
    #[cfg(not(target_arch = "wasm32"))]
    let builder = builder.timeout(std::time::Duration::from_secs(timeout_secs));
    builder.build().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urlencode_escapes_query_chars() {
        assert_eq!(urlencode("Kanye West"), "Kanye+West");
        assert_eq!(urlencode("R&B/Hip-Hop"), "R%26B%2FHip-Hop");
        assert_eq!(urlencode("abc-_.~09"), "abc-_.~09");
    }
}
