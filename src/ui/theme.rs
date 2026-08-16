/// Design tokens mirrored in `assets/main.css`. Kept as constants so Rust code
/// (e.g. placeholder generation) can reference them on the syntax side too.
pub const BG: &str = "#0a0e1a";
pub const SURFACE: &str = "#161c30";
pub const CARD: &str = "#1a2136";
pub const BLUE: &str = "#3b82f6";
pub const BLUE_2: &str = "#60a5fa";
pub const CYAN: &str = "#22d3ee";
pub const TEXT: &str = "#f1f5fb";
pub const MUTED: &str = "#9aa6c3";
pub const RADIUS: &str = "10px";
pub const SIDEBAR_WIDTH: &str = "240px";
pub const PLAYER_HEIGHT: &str = "92px";

/// Allowed, whitelisted artwork hosts rendered by `<img>`.
pub fn is_allowed_media_url(url: &str) -> bool {
    crate::adblock::extract_host(url)
        .map(|host| host.ends_with("scdn.co") || host.ends_with("spotifycdn.com"))
        .unwrap_or(false)
}