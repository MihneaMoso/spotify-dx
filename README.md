# Spotify DX

A cross-platform Spotify client written in Rust + [Dioxus](https://dioxuslabs.com)
that behaves like **open.spotify.com**: it signs you in through the real Spotify
login page inside the app, keeps you logged in across restarts, and renders the
whole UI as fast native components. On **premium** accounts it plays full tracks
via the Web Playback SDK; on **free** accounts it browses/search plays and
explains that playback needs Premium. An in-process AdGuard blocklist drops
third-party ad/tracker requests.

## How it works

1. **First launch** opens a GTK window hosting `open.spotify.com`. You log in
   exactly as you would in a browser (password / 2FA / passkey).
2. The login WebView shares a persistent data directory (`webview_session/`).
   Its session cookies (`sp_dc`, …) are what keep you logged in forever after —
   exactly like the web app.
3. Injected JS polls Spotify's internal `get_access_token` endpoint to detect
   the login and capture the short-lived **web-player access token**. It's
   mirrored to the OS keychain so startup can restore a session without a window.
4. On **desktop**, playback is driven by the [Web Playback SDK](https://developer.spotify.com/documentation/web-playback-sdk)
   running in a hidden wry WebView that reuses the same session cookies, so no
   token hand-over is needed.
5. Every outbound request goes through an in-process **AdGuard blocklist trie**
   (`adblock/`); third-party ad/tracker hosts are dropped before hitting the
   network.

## Feature checklist

- [x] open.spotify.com-style login: real Spotify sign-in in a GTK WebView, session cookies persisted
- [x] Persistent sessions (no re-login across app restarts), token refresh via the web-player endpoint
- [x] AdGuard DNS-filter blocklist (merged, trie-indexed, bundled snapshot + background update)
- [x] Web Playback SDK boot via hidden WebView (desktop) with shared cookie jar
- [x] Player bar: play/pause, next/prev, seek, volume, shuffle/repeat
- [x] Pages: Home (featured + new releases + recommended), Search (debounced), Library, Playlist, Album, Artist
- [x] Artwork pipeline: colored placeholder → blur-up → full (downscaled, base64, thumbnails never hit the disk)
- [x] Mobile responsive shell (bottom nav swaps with the side nav)

## Build

Prerequisites: Rust 1.75+, and on Linux `pkg-config`, `libwebkit2gtk-4.1-dev` (±`libappindicator3-dev`, `librsvg2-dev`).

```bash
# Linux/macOS desktop (default) — no credentials needed
cargo build --release
cargo run --release

# Web (Connect API only)
cargo build --no-default-features --features web

# Mobile (Connect API only)
cargo build --no-default-features --features mobile
```

No `SPOTIFY_CLIENT_ID` or Spotify Developer app is required — the app uses the
same web session that open.spotify.com uses.

## Configuration

| Variable | Default | Purpose |
| --- | --- | --- |
| `SPOTIFY_DX_LOG` | `wry=warn,tao=warn,tokio=info,spotify_dx=info` | Tracing filter |

## Data & directories

| Path | What lives there |
| --- | --- |
| OS keychain | Web-player access token + expiry |
| `data_local_dir/spotify-dx/webview_session/` | WebView session cookies (`sp_dc`, …) — keeps you logged in |
| `cache_dir/blocklist_cache.txt` | Merged snapshot of the blocklists |
| `assets/blocklist_cache.txt` | Bundled snapshot shipped with the app (cold-start) |

## Layout

```
src/
  adblock/     trie-based blocklist + AdGuard/Hosts parsing + background refresh
  auth/        webview_login.rs (open.spotify.com sign-in window), keychain token store, boot auth
  player/      Web Playback SDK bootstrap (desktop) / Connect API fallback
  spotify/     API client, models, playback API
  ui/          pages, components, router, theme, inline icons
  app.rs       login gate / routed shell
  main.rs      per-renderer entry points + bootstrap
assets/
  main.css             design system
  blocklist_cache.txt  bundled blocklist snapshot
  icons/*.svg          brand/menu icons
```

## Tests

```bash
cargo test                      # unit tests (network-free)
cargo test --no-default-features  # headless tooling build (also compiles the CI target)
```

## Design

Contrast-first dark theme, Spotify green accent, 240px side rail, persistent 90px now-playing
bar, hover-to-reveal sliders, and a mobile shell that swaps the rail for a bottom `Home /
Search / Library` nav.

## Credits & notes

- Blocklist: AdGuard DNS filter (`adguardteam.github.io/AdGuardSDNSFilter/Filters/filter.txt`)
- Spotify artwork is served from `i.scdn.co`, which is explicitly **never** blocked.
- Spotify's own domains (`*.spotify.com`, `*.spotifycdn.com`, `*.scdn.co`) are **never**
  blocked — the login, token endpoint, API and audio streams all live there. The blocklist
  targets genuinely third-party ad/tracker hosts.
