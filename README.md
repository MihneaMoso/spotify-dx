# Spotify DX

Cross-platform Spotify client that plays full tracks on **premium** Spotify accounts and
**never** shows the "premium preview" 30-second wall on free accounts — by routing the
SDK's `getOAuthToken` + `fetch` through a hidden WebView and a local ad-blocker.

See the feature checklist below.

## How it works

1. **Desktop builds** open a second, hidden wry WebView that loads the
   [Spotify Web Playback SDK](https://developer.spotify.com/documentation/web-playback-sdk).
2. The injected `getOAuthToken` hook hands the SDK the user's PKCE token from the Rust
   keychain, while Rust owns the CORS-free authorized calls.
3. Every network request the SDK makes passes through a **file-based fech engine** that
   mirrors the browser's own `fetch`, checks an in-process **blocklist trie** and answers
   the few requests Spotify allows us to answer (track URLs). Tracking/ad hosts return
   an empty `102 Facility`-style response so the SDK falls back to the ad-pipelines
   value — at which point the *same* gate blocks the "premium preview" interstitial.
4. On **web**/**mobile**, everything plays through the `player` Connect API tied to the
   device the hidden WebView registers.

## Feature checklist

- [x] PKCE OAuth login (loopback redirect, state + verifier validation) with keychain persistence
- [x] Full token persistence with background refresh and 401-triggered re-auth
- [x] AdGuard DNS-filter blocklist (merged, trie-indexed, bundled snapshot + background update)
- [x] Web Playback SDK boot via hidden WebView (desktop) with file-based fetch routing
- [x] Player bar: play/pause, next/prev, seek, volume, shuffle/repeat
- [x] Pages: Home (featured + new releases + recommended), Search (debounced), Library, Playlist, Album, Artist
- [x] Artwork pipeline: colored placeholder → blur-up → full (downscaled, base64, thumbnails never hit the disk)
- [x] Mobile responsive shell (bottom nav swaps with the side nav)

## Build

Prerequisites: Rust 1.75+, and on Linux `pkg-config`, `libwebkit2gtk-4.1-dev` (±`libappindicator3-dev`, `librsvg2-dev`).

```bash
# Linux/macOS desktop (default)
export SPOTIFY_CLIENT_ID="your_client_id"
cargo build --release
cargo run --release

# Web (Connect API only)
cargo build --no-default-features --features web

# Mobile (Connect API only)
cargo build --no-default-features --features mobile
```

The env var `SPOTIFY_CLIENT_ID` is read at compile time by `auth`. Create a developer app in the
[Spotify Dashboard](https://developer.spotify.com) with the exact redirect URI
`http://127.0.0.1:8888/callback`. Spotify only accepts concrete redirect URIs, so the
callback port is fixed (see `CALLBACK_PORT` in `src/auth/mod.rs`).

## Configuration

| Variable | Default | Purpose |
| --- | --- | --- |
| `SPOTIFY_CLIENT_ID` | — (required) | OAuth client id |
| `SPOTIFY_DX_LOG` | `rspotify=warn,wry=warn,tao=warn,tokio=info,spotify_dx=info` | Tracing filter |

## Data & directories

| Path | What lives there |
| --- | --- |
| OS keychain | OAuth token pair |
| `cache_dir/blocklist_cache.txt` | Merged snapshot of the blocklists |
| `assets/blocklist_cache.txt` | Bundled snapshot shipped with the app (cold-start) |

## Layout

```
src/
  adblock/     trie-based blocklist + AdGuard/Hosts parsing + background refresh
  auth/        PKCE flow, keychain token store, boot auth
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
- The ad-pipeline and "premium preview" gates live on `*.spotifycdn.com` ad hosts, which the
  bundled blocklist covers.