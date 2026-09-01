# Spotify DX

A cross-platform Spotify client written in Rust + [Dioxus](https://dioxuslabs.com)
that behaves like **open.spotify.com**: it signs you in through the real Spotify
login page inside the app, keeps you logged in across restarts, and renders the
whole UI as fast native components. **Every signed-in user gets full-track
playback** — Premium accounts use the official Web Playback SDK, free accounts
use an open multi-source engine (TIDAL/Qobuz/YouTube community backends). An
in-process AdGuard blocklist drops third-party ad/tracker requests.

## How it works

1. **First launch** opens a GTK window hosting `open.spotify.com`. You log in
   exactly as you would in a browser (password / 2FA / passkey).
2. The login WebView shares a persistent data directory (`webview_session/`).
   Its session cookies (`sp_dc`, …) are what keep you logged in forever after —
   exactly like the web app.
3. Injected JS polls Spotify's internal `get_access_token` endpoint to detect
   the login and capture the short-lived **web-player access token**. It's
   mirrored to the OS keychain so startup can restore a session without a window.
4. On **desktop and mobile** (all native wry renderers), playback is driven by
   the [Web Playback SDK](https://developer.spotify.com/documentation/web-playback-sdk)
   running in a hidden wry WebView that reuses the same session cookies, so no
   token hand-over is needed. **Free accounts** get full-track playback through
   an open multi-source engine (TIDAL/Qobuz/YouTube community backends) instead.
5. Every outbound request goes through an in-process **Brave-style ad-block
   engine** (`adblock/`); third-party ad/tracker hosts are dropped before
   hitting the network. The engine runs on a dedicated thread and checks URLs
   via channel IPC.

## Feature checklist

- [x] open.spotify.com-style login: real Spotify sign-in in a GTK WebView, session cookies persisted
- [x] Persistent sessions (no re-login across app restarts), token refresh via the web-player endpoint
- [x] Brave-style ad-block engine (adblock crate, dedicated thread, serialized cache, background refresh)
- [x] Request store: in-flight coalescing, memory TTL cache, disk stale-while-revalidate
- [x] Disk-cached artwork (SHA-256 keyed, 128-file LRU, 30-day TTL)
- [x] Web Playback SDK boot via hidden WebView (desktop) with shared cookie jar
- [x] Open multi-source engine (TIDAL/Qobuz/YouTube) for free-tier full-track playback
- [x] Player bar: play/pause, next/prev, seek, volume, shuffle/repeat
- [x] Pages: Home (featured + new releases + recommended), Search (debounced), Library, Playlist, Album, Artist
- [x] Artwork pipeline: colored placeholder → blur-up → full (downscaled, base64, thumbnails never hit the disk)
- [x] Mobile responsive shell (bottom nav swaps with the side nav)

## Build

Prerequisites: Rust 1.75+, and on Linux `pkg-config`, `libwebkit2gtk-4.1-dev` (±`libappindicator3-dev`, `librsvg2-dev`, `libxdo-dev` — `libxdo-dev` is required for the X11 clipboard/input link, don't omit it on CI).

```bash
# Linux/macOS desktop (default) — no credentials needed
cargo build --release
cargo run --release

# Web (WASM) — bundles to `dist/`; login/playback still live-validation-pending
cargo build --no-default-features --features web

# Mobile (Android/iOS native — wry webview login + Web Playback SDK playback)
cargo build --no-default-features --features mobile
```

No `SPOTIFY_CLIENT_ID` or Spotify Developer app is required — the app uses the
same web session that open.spotify.com uses.

## Install

Prebuilt binaries are published to the [GitHub Releases](https://github.com/MihneaMoso/spotify-dx/releases)
for every version tag. The easiest way to install the latest release is the
curl-able installer:

```sh
curl -fsSL https://raw.githubusercontent.com/MihneaMoso/spotify-dx/master/install.sh | bash
```

What it does, by platform:

- **Linux / macOS** — downloads the release binary and installs it to
  `~/.local/bin/spotify-dx` (add that to your `PATH` if needed).
- **Windows** — downloads `spotify-dx.exe` into `%LOCALAPPDATA%\Programs\SpotifyDX\`.
- **Android / Termux** — downloads the APK into your `~/Download` folder; open
  it on the device to install (allow *Install from unknown sources* if prompted).

Every download is verified against the SHA-256 digest published on the release
(`shasum -a 256` / `sha256sum`). Env overrides:

| Env var | Meaning |
|---|---|
| `SPOTIFY_DX_VERSION` | Pin a version instead of the latest (`v0.1.9`) |
| `SPOTIFY_DX_PREFIX` | Install to a custom prefix instead of the default |
| `SPOTIFY_DX_DRYRUN=1` | Print what would be downloaded/installed without touching disk |

## Web app & landing page

There's also a web build of the app, deployed from CI to GitHub Pages when you
push to `master`:

- Landing page: https://mihneamoso.github.io/spotify-dx/
- Web app: https://mihneamoso.github.io/spotify-dx/app/

Both are built from source in CI (`scripts/build-web.sh`, shared with the
release workflow — every version tag also redeploys the web build). The app is
built with `base_path = "spotify-dx/app"` in `Dioxus.toml` so it works under the
`/spotify-dx/` path prefix; the landing page in `web/site/` uses only relative
URLs. To build and inspect the deploy tree locally:

```sh
bash scripts/build-web.sh   # assembles ./_deploy (requires `dx` CLI + wasm32 target)
```

## Releases

Desktop binaries for Linux (glibc, x86_64), macOS (arm64 + x86_64) and Windows
(x86_64) are built in GitHub Actions and published as a GitHub Release whenever a
version tag is pushed. The pipeline lives in `.github/workflows/release.yml` and
is adapted from the reference `magic-run` project — with one key difference:
Spotify DX is a GTK/WebKit GUI, so Linux uses the system WebKit dev packages
(glibc), **not** a musl static build.

Trigger a release automatically:

```bash
git tag v0.1.0
git push origin v0.1.0
```

or use the manual script (validates semver, tags, pushes, and waits for CI):

```bash
./scripts/release.sh 0.1.1          # release version 0.1.1 (tag v0.1.1)
./scripts/release.sh --dry-run 0.1.1  # show the plan without doing anything
```

Assets per release target:

| Asset | Platform |
| --- | --- |
| `spotify-dx-<version>-x86_64-unknown-linux-gnu.tar.gz` (+ alias `spotify-dx-<target>.tar.gz`) | Linux |
| `spotify-dx-<version>-aarch64-apple-darwin.tar.gz` / `...-x86_64-apple-darwin.tar.gz` (+ aliases) | macOS |
| `spotify-dx-<version>-x86_64-pc-windows-msvc.zip` (+ alias `spotify-dx-<target>.zip`) | Windows |
| `spotify-dx-<version>-signed.apk` | Android (aarch64, generated keystore) |
| `spotify-dx-<version>-web.tar.gz` (+ alias `spotify-dx-web.tar.gz`) | Web (WASM bundle) |

Each asset ships with a `.sha256` checksum, and each target has an unversioned
alias so `…/releases/latest/download/…` always fetches the newest build.

> **Note on web / Android / iOS:** mobile now builds the same native app as
> desktop (in-app `open.spotify.com` login via the wry webview + Web Playback
> SDK playback). The **web (WASM)** renderer still bundles, but its
> credentialed cross-origin `get_access_token` login is live-validation-pending
> and falls back to the Connect API until confirmed in a real browser — see
> `docs/PLATFORM_PARITY.md`.

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
| `cache_dir/adblock_engine.bin` | Serialized ad-block engine (fast restart) |
| `assets/blocklist_cache.txt` | Bundled snapshot shipped with the app (cold-start) |

## Layout

```
src/
  adblock/     Brave ad-block engine (adblock crate) + blocklist fetch + cosmetic CSS scaffold
  auth/        webview_login.rs (open.spotify.com sign-in window), keychain token store, boot auth
  media/       audio.rs (symphonia decode), images.rs (disk-cached artwork), sink.rs (rodio audio output)
  player/      PlaybackEngine trait, SDK bootstrap (native wry renderers) / Connect API fallback
  streaming/   Open engine: provider trait, TIDAL/Qobuz/YouTube, resolver, Odesli ID mapping, URL cache
  spotify/     API client, models, request store (coalescing + SWR), playback API
  ui/          pages, components, router, theme, inline icons
  app.rs       login gate / routed shell
  main.rs      per-renderer entry points + bootstrap
  settings.rs  persistent settings (theme, volume, engine preference, cosmetic toggle)
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

- Blocklist: AdGuard DNS filter (`adguardteam.github.io/AdGuardSDNSFilter/Filters/filter.txt`) + curated Spotify ad/analytics rules, loaded via the Brave `adblock` crate.
- Spotify artwork is served from `i.scdn.co`, which is explicitly **never** blocked.
- Spotify's own domains (`*.spotify.com`, `*.spotifycdn.com`, `*.scdn.co`) are **never**
  blocked — the login, token endpoint, API and audio streams all live there. The blocklist
  targets genuinely third-party ad/tracker hosts.
