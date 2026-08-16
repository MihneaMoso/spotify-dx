/// Design tokens mirrored in `assets/main.css`. Kept as constants so Rust code
/// (e.g. placeholder generation) can reference them on the syntax side too.
pub const BG: &str = "#0d0d0d";
pub const SURFACE: &str = "#181818";
pub const CARD: &str = "#242424";
pub const GREEN: &str = "#1db954";
pub const GREEN_2: &str = "#1ed760";
pub const TEXT: &str = "#ffffff";
pub const MUTED: &str = "#b3b3b3";
pub const RADIUS: &str = "8px";
pub const SIDEBAR_WIDTH: &str = "240px";
pub const PLAYER_HEIGHT: &str = "90px";

/// Allowed, whitelisted artwork hosts rendered by `<img>`.
pub fn is_allowed_media_url(url: &str) -> bool {
    crate::adblock::extract_host(url)
        .map(|host| host.ends_with("scdn.co") || host.ends_with("spotifycdn.com"))
        .unwrap_or(false)
}