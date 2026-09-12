# RULES.md — Spotify DX: full agent reference

The authoritative, long-form companion to `AGENTS.md`. Read this first. When
something important changes (a gotcha, a discovery, a new convention, a major
migration), update this file in the same change.

## 1. Ground rules (non-negotiable)

1. **NEVER run, stop, restart, or kill `dx serve` or the app binary yourself.**
   The user runs the dev loop manually and has `dx serve` configured for
   auto-reload. Do NOT:
   - start `dx serve`
   - kill the user's `dx serve` or app process
   - run `cargo run`
   Doing so wastes CPU cycles, clobbers the user's own instance, and can spawn
   duplicate servers / duplicate windows.
   If the user reports something looks stale (e.g. CSS not applied), check the
   CSS/code itself first — do not assume the dev server needs a restart.
2. **Do not touch build artifacts** under `target/`. The user's `dx serve`
   manages `target/dx`; `cargo` manages `target/debug` etc. Leave both alone.
3. **Record important changes here.** When you learn something that would have
   saved you time — a gotcha, a migration note, a design decision — add it to
   the relevant section of this file in the same commit/change.
4. **Never commit unless explicitly asked.**

## 2. Verify with cargo (never run the app)

The user's `dx serve` instance handles the running app. You verify code with:

```bash
cargo check --features desktop      # primary check
cargo clippy --features desktop     # lint (expect zero warnings)
cargo build --features desktop      # if you need a real binary (no run!)
cargo test                          # unit tests, network-free
cargo test --no-default-features    # also compiles the headless/CI target
```

`cargo check --tests` and `cargo check` (default features) are useful
complements. Never `cargo run` and never launch the produced binary.

## 3. Environment prerequisites

- **No API credentials are needed.** Auth is the web-player session: the user
  signs in on `open.spotify.com` inside a GTK WebView and the app captures the
  access token from Spotify's internal `get_access_token` endpoint. There is no
  OAuth app, so `SPOTIFY_CLIENT_ID` is **not** read anywhere (the old PKCE flow
  was deleted).
- Linux desktop builds need `pkg-config`, `libwebkit2gtk-4.1-dev`
  (±`libappindicator3-dev`, `librsvg2-dev`), and **`libxdo-dev`** (provides
  `libxdo.so`, which `libxdo-sys` links unconditionally via the dioxus X11
  clipboard/input stack — missing it fails the *link* stage with
  `unable to find library -lxdo`, as it did in CI; the release workflow's Linux
  dependency install includes it).
- **Audio stack (Phase-0 decision):** decoding = `symphonia 0.6` (features:
  flac,aac,mp3,isomp4,ogg,pcm); sound-card sink = `rodio 0.22`.
  **rodio MUST stay `default-features = false, features = ["playback"]`** — its
  default decoder features pull in symphonia 0.5, which would duplicate the entire
  codec stack alongside our direct 0.6 dependency.
- Symphonia 0.6 broke hard from 0.5 (learned the expensive way): `Probe::probe()`
  returns `Box<dyn FormatReader>` directly (no `ProbedMetadata`); `MediaSource` needs
  `is_seekable()`/`byte_len()` (no `len()`); decoders are per-media-type
  (`CodecRegistry::make_audio_decoder(&AudioCodecParameters, &AudioDecoderOptions)`);
  `Track.codec_params` is an `Option<CodecParameters>` enum (`is_audio()` /
  `.audio()`); `next_packet()` returns `Result<Option<Packet>>`; packet fields are
  public (`packet.track_id`, `packet.pts.get()`); `TimeBase` fields are
  `numer`/`denom` (`NonZero<u32>`, not num/den). Reference implementation lives in
  `src/media/audio.rs`.
- The dev loop: user runs `dx serve` (dx CLI 0.7.x). It is **auto-reload**, so
  code changes are picked up without a restart.

## 4. Project agent-map

### 4.1 What it is

A cross-platform Spotify client ("Spotify DX") written in Rust with Dioxus. It
behaves like open.spotify.com: the first launch opens the real Spotify sign-in
in a WebView, captures the web-player session, and stays logged in across
restarts via the persisted session cookies. On **premium** accounts it plays
full tracks through the Web Playback SDK driven from a hidden wry WebView; on
**free** accounts it plays full tracks through an open multi-source engine
(TIDAL/Qobuz/YouTube community backends). An in-process ad-blocker (AdGuard DNS
lists) drops third-party ad/tracker requests.

### 4.2 High-level data flow

1. `main.rs::bootstrap()` (tokio, pre-window): starts the ad-blocker, restores
   the auth session from the OS keychain, and hands it to dioxus via
   `auth::set_boot_auth()`.
2. `app.rs::App`: login gate vs. `Router::<Route>` shell, depending on
   `AUTH_STATE`.
3. `ui/router.rs::Route`: routes wrapped in `AppLayout` (persistent shell:
   side/bottom nav + player bar + toasts).
4. Playback: `player::` — native renderers (desktop + mobile) use the hidden
   wry WebView running the SDK via the shared `native` feature; only web (WASM)
   falls back to the Spotify Connect API (still live-validation-pending).
5. Every outbound request goes through `spotify::client::filtered_get` /
   `filtered_get_auth`, which consult the ad-block trie before sending.

### 4.3 Module map (`src/`)

| Path | Responsibility |
| --- | --- |
| `main.rs` | Per-renderer entry points (`desktop`/`web`/`mobile`/headless) + shared `bootstrap()`, logging init. Desktop sets up the window via `dioxus::desktop::Config` + `WindowBuilder` and launches with `LaunchBuilder::desktop()`. Applies a staged update before the runtime starts (no signal writes pre-runtime). |
| `app.rs` | Root `App` component: injects the stylesheet, login gate vs. routed shell, seeds auth from boot snapshot, calls `player::init()` + `player::on_authenticated()` once. One-time startup auto-update check when `SETTINGS.auto_check_updates` is on. |
| `updater.rs` | Self-update (kal-style): polls the latest `MihneaMoso/spotify-dx` GitHub release, matches a platform asset by substring token, downloads it streaming with SHA-256 verification, stages a tar.gz on desktop (flate2+tar) or the APK on Android (filesDir via safe JNI `dispatch`), then swaps the binary on next launch or fires the Android package installer. `GlobalSignal`s: `UPDATE_STATUS` (diagnostic text) + `UPDATE_READY`. Pure helpers `parse_version`/`is_newer`/`pick_asset` are unit-tested. Wasm builds out. |
| `profile.rs` | Local user profile: `UserProfile { username, avatar_mime, avatar_b64 }` persisted to `{data_dir}/profile.json`. `PROFILE`/`PROFILE_ERROR` globals; avatar accepts common image magic bytes, capped at 4 MiB, base64 data-URI. |
| `build.rs` | Injects `SPOTIFY_DX_VERSION` (`SPOTIFY_DX_RELEASE_VERSION` env → `git describe --tags` → Cargo version). `updater::CURRENT_VERSION` reads it; leading `v` stripped. |
| `state.rs` | Global signals (single source of truth): `APP_STATE`, `PLAYER_STATE`, `AUTH_STATE`, `ADBLOCK_STATS`, `APP_ERROR`. Plus `Page`, `RepeatMode`, and the state structs. |
| `app_error.rs` | `AppError` enum (thiserror). |
| `auth/` | Web-session sign-in: `webview_login.rs` (desktop GTK window hosting `open.spotify.com`, cookie capture via the internal `get_access_token` endpoint), keychain persistence (`token_store.rs`), refresh/init flows (`mod.rs`). |
| `spotify/` | API models (`models.rs` — incl. `SavedTrack` envelope for `/me/tracks`), filtered HTTP client (`client.rs`), endpoints (`api.rs` — thin wrappers delegating to GQL), GraphQL persisted-query client (`gql.rs` — `api-partner.spotify.com/pathfinder`, used for user playlists + liked songs + saved albums + home + search + album/artist detail), playback endpoint (`player_api.rs`), session helpers (`session.rs`), request store (`store.rs` — in-flight coalescing, memory TTL, disk SWR). |
| `adblock/` | Brave-style ad-block engine (`engine.rs` — `adblock` crate `Engine` on a dedicated `!Send` thread with `mpsc` channel IPC), blocklist fetch/cache (`adguard_api.rs`), cosmetic CSS scaffold (`mod.rs::cosmetic`). Facade: `should_block(url)`, `record_drop()`, `stats_snapshot()`. |
| `player/` | `mod.rs` dispatch (native renderers → webview_bridge; wasm → Connect API), `playback_sdk.rs` (embedded SDK HTML/JS), `webview_bridge.rs` (hidden WebView + IPC), `engine.rs` (`PlaybackEngine` trait). `should_use_open_engine()` checks `EnginePreference`; `play_uri()` routes to `open_play_uri()` via the streaming engine. |
| `ui/` | `router.rs` (incl. `/liked`, `/queue`, `/settings`), `theme.rs` (tokens mirrored from CSS + drift-guard tests incl. the custom-property linter), `icons.rs` (inline SVG), `components/`, `pages/`. |
| `ui/components/` | `app_layout.rs` (shell + sidebar resize), `top_bar.rs` (history/search/user menu — avatar image or initial + name from `PROFILE`), `nav.rs` (`SideNav`/`BottomNav`), `now_playing.rs` (right column), `player_bar.rs`, `progress_bar.rs`, `primitives.rs` (`SectionHeader`/`HeroHeader`/`TrackTable`/`SkeletonShelves`), `album_art.rs`, `card.rs` (MediaCard w/ `extra_class`), `track_row.rs`, `toast.rs`. |
| `ui/pages/` | `login.rs`, `home.rs`, `search.rs`, `library.rs`, `liked.rs`, `queue.rs`, `settings.rs` (incl. Profile section — name + avatar upload — and Software & updates section), `playlist.rs`, `album.rs`, `artist.rs`. |
| `media/` | `audio.rs`: symphonia decode (FLAC/M4A/MP3/OGG, seek, gapless planner) with tests. `images.rs`: disk-cached artwork loader (SHA-256 keyed, 128-file LRU, 30-day TTL). `sink.rs`: rodio audio sink thread (`MixerDeviceSink` + `Player` via `rodio::play()`), `SinkCommand` channel, `SinkState` atomics. |
| `streaming/` | Open streaming engine (Phase 4b). `provider.rs`: `Provider` trait, `Resolution` enum, `TrackQuery`. `odesli.rs`: song.link ID mapping (DEAD — public API sunset/401). `cache.rs`: stream-URL cache (memory + disk, 50-min TTL, FIFO 256). `resolver.rs`: cache → provider failover. `providers/{tidal,qobuz,youtube}.rs`: TIDAL & Qobuz DISABLED (`is_available()==false`, Odesli sunset); YouTube (InnerTube ANDROID API) is the sole active, self-contained provider. |
| `settings.rs` | Persistent user settings (`{data_dir}/settings.json`): theme, volume, engine preference (`EnginePreference` enum: Auto/SpotifySdk/Open), `hide_upsell` toggle, `auto_check_updates` (default `true`). Load failures always fall back to defaults. Exposed app-wide as `state::SETTINGS`. |
| `util.rs` | Shared helpers. |

### 4.4 Key files & ideas to know

- **`src/ui/components/app_layout.rs`** — the app-shell grid + the sidebar
  drag-to-resize mechanism. The `sidebar_width`/`resizing` signals live here;
  `onpointermove`/`up`/`cancel` are on the app-shell div, and the resizer
  handle only starts the drag (with JS `setPointerCapture` via
  `dioxus::document::eval`). Do not "simplify" this pattern.
- **`src/player/webview_bridge.rs`** — the hidden WebView hosting the SDK.
  Uses a `thread_local!` `WebView` (webkitgtk must be touched only on the UI
  thread) and a `tokio::sync::mpsc` IPC queue drained inside a dioxus task.
  Direct signal access from the IPC handler panics (no dioxus runtime there).
- **`assets/main.css`** — the whole design system. Organized in numbered
  sections (`/* ── 3. App shell layout ── */`, etc.). When editing layout,
  check BOTH the base rules and the `@media (max-width: 820px)` block — see
  the sidebar-resizer / bottom-nav gotcha below.
- **`src/ui/theme.rs`** — design tokens duplicated as constants; keep in sync
  with the CSS variables (e.g. `PLAYER_HEIGHT` ↔ `--player-height`).
- **`src/state.rs`** — `Signal::global` statics; cross-component state flows
  through these, not props drilling.
- **`src/auth/webview_login.rs`** — the in-window sign-in. A WebView packed
  into the main window's `vbox` shows `open.spotify.com` (the dioxus UI is
  hidden underneath); an injected poller waits for the web-player access token
  and hands it to Rust over `window.ipc.postMessage`. Built with the shared
  session `WebContext` (`auth::with_session_context`), so the session cookies
  persist across restarts. Never reparents WebViews (see §6.8).

### 4.5 Router & features

- `src/ui/router.rs`: `Route` enum with `#[layout(AppLayout)]` and
  `#[route(...)]`. Pages: `/`, `/search`, `/library`, `/album/:id`,
  `/artist/:id`, `/artist/:id/top`, `/playlist/:id`.
- Cargo features: `default = ["desktop"]`; `desktop`, `mobile`, `web`, plus a
  shared `native` feature (`dep:wry`) enabled by **both** `desktop` and `mobile`
  — `dioxus::mobile` is a re-export of `dioxus::desktop` (both wry-based), so
  "native non-WASM" code (media sink, adblock, `playback_sdk` bootstrap) is
  gated on `#[cfg(feature = "native")]` and compiles for desktop and mobile
  alike. Only WASM (`web`) diverges.
- `#![forbid(unsafe_code)]` at crate level — the whole crate is unsafe-free,
  and the WebView bridge is deliberately `Send`-free via `thread_local!` to
  keep it that way. Keep it that way.

## 5. Conventions

- No code comments unless they explain *why*; the codebase uses sparse,
  high-value comments. Match that.
- Design tokens live in both `ui/theme.rs` and `assets/main.css` — keep them
  in sync.
- Global state via `Signal::global` in `state.rs`; components read/write these.
- Errors via `AppError` (`app_error.rs`); user-facing errors go through
  `state::publish_error` → `Toast` component.
- Any new page: add the module in `ui/pages/`, export it in `pages/mod.rs`,
  add the route in `ui/router.rs`. Components similarly via
  `ui/components/mod.rs`.
- Formatting follows `cargo fmt` defaults; edition 2021.
- **Releases run through `.github/workflows/release.yml`, triggered by a
  `v*` tag push (or `./scripts/release.sh <ver>`).** It builds the `desktop`
  feature for linux-gnu / macOS (arm64+x86_64) / windows-msvc, the owned
  Kotlin Android APK (`android-apk` job), and the web bundle — and publishes
  a GitHub Release. Because this is a GTK/WebKit GUI, Linux is **glibc with the
  system webkit2gtk dev packages**, never musl. (The old
  `docs/old/PLATFORM_PARITY.md` Connect-only matrix no longer applies:
  Android has full open-engine playback via the Kotlin app.)## 6. Discoveries & gotchas (learned the hard way)

### 6.1 The dioxus 0.6 → 0.7 migration (important!)

The project was migrated from `dioxus 0.6` to `dioxus 0.7.10` to match the
installed `dx` CLI 0.7.10 (a 0.6 binary under a 0.7 dev server produces an
unstyled app / version-incompatibility warning). Relevant notes:

- `Cargo.toml` pins: `dioxus = "0.7"` (feature `router`), `dioxus-router = "0.7"`,
  `wry = "0.53"`. **The wry version must match the one dioxus-desktop 0.7.10
  already depends on (0.53.x)** — otherwise cargo resolves two wry copies into
  the binary. If you bump dioxus, re-check wry.
- **`peek()` moved to the `ReadableExt` trait.** Code that used
  `use dioxus::signals::Readable;` for `.peek()` must import
  `dioxus::prelude::ReadableExt` instead (see `spotify/session.rs`,
  `player/mod.rs`, `player/webview_bridge.rs`). `.read()`/`.write()` still come
  from the prelude.
- **wry 0.53 removed `WebViewBuilder::new_gtk(vbox)`.** The Linux path is now
  `WebViewBuilder::new()` + platform options, then
  `builder.build_gtk(vbox)` (returns `Result<WebView>`) via
  `wry::WebViewBuilderExtUnix`. Non-Linux uses `builder.build(&window)`.
  See `player/webview_bridge.rs::init`.
- `asset!()` (manganis) API is unchanged in 0.7; `assets/main.css` is loaded
  via `Link { rel: "stylesheet", href: asset!("/assets/main.css") }` in
  `app.rs`.
- `use_coroutine` in 0.7 still uses `futures_channel::mpsc::UnboundedReceiver`
  (`futures::channel::mpsc::UnboundedReceiver` is the same type) — the existing
  imports are correct; `.next().await` needs `futures::stream::StreamExt`.

### 6.2 CSS is compiled into the binary

`asset!()` embeds the stylesheet at **compile time**. Editing `assets/main.css`
alone will NOT change a running binary. However — **the user's `dx serve` is
auto-reload and handles rebuilds itself; never restart it on your own.** If you
suspect stale CSS, inspect the code/CSS for the actual bug first. (One real
bug found this way: a later `.bottom-nav { position: fixed; bottom: ... }`
rule silently overriding an earlier `display: none` in the same sheet.)

### 6.3 The shell grid (current layout, Phase 2)

`.app-shell` is a 4-row × 3-column grid on desktop:
`top top top / sidenav main np / player player player / nav nav nav`.
The now-playing column participates through `--np-width`, bound INLINE by
`AppLayout` (0 px = hidden; CSS drops the column entirely below 1280 px).
Breakpoints: ≤1279 px no np column · ≤999 px fixed 72 px icon rail (resizer
hidden — the inline `--sidebar-width` would fight it) · ≤820 px stacked
mobile layout with bottom nav. When changing this:
- Keep the grid-area assignments in `assets/main.css` section 3 consistent
  (`ui/theme.rs` tests assert all six zones + the `"sidenav main   np"` row).
- `.sidebar-resizer` is `position: fixed`; its `top` is `var(--topbar-height)`
  and its `bottom` must equal
  `calc(var(--player-height) + var(--bottom-nav-height))`.
- `.toast` floats above the player bar and must also clear the bottom-nav row.
- The theme-sync test `every_css_custom_property_in_use_is_defined` lints the
  stylesheet: bare `var(--x)` references need a definition; inline-bound vars
  (e.g. `--np-width`) must carry a fallback.

### 6.4 Desktop window & "wide vs narrow" media queries

The desktop window defaults to 1200×780. That sits between the breakpoints:
base CSS governs layout, the now-playing column is hidden (1200 < 1280), and
the rail is full-width (1200 > 999). When a "mobile-only" rule (e.g.
`.bottom-nav`) is involved, verify which rule actually wins in the base
sheet — a later same-specificity rule overrides an earlier `display: none`.

### 6.5 Dioxus 0.7 rsx gotchas (learned in Phase 2/3 — follow the house pattern)

- **No `let` bindings inside rsx `for` loop bodies** ("expected identifier").
  Precompute owned tuples in plain Rust *before* the rsx block and iterate
  those (see every page: `for (id, title, …) in cards`).
- **Event handlers must return `()`.** `navigator.push(..)` returns a value —
  wrap it: `onclick: move |_| { navigator.push(Route::…); }`.
- **Never hold a resource read-guard across an rsx `return`** (E0597
  "does not live long enough" when a nested `use_resource` follows, or when
  different branches return early). Clone the payload out first:
  `let loaded = resource.read().as_ref().and_then(|r| r.as_ref().ok()).cloned();`
- **Two closures cannot both move the same Vec** (E0382) — give each its own
  clone (`let pool = data.clone();`) or precompute per-closure values.
- **Signal handles are `Copy`; they do not need `mut`.** Only signal-derived
  write paths need mutation.
- Shared fetch helpers should be plain `fn`s taking a `#[derive(Clone)]`
  context struct of signals (see `liked.rs::fetch_page`), NOT closures —
  closures get moved into effects/buttons and each use site needs another clone.
- Prefer `*signal.write() = v;` over `.set()` if trait imports are unclear;
  `peek()` requires `use dioxus::prelude::ReadableExt;`.

### 6.6 WebView / SDK pitfalls

- The hidden WebView runs the Web Playback SDK; on free accounts the SDK
  reports `init_error: Failed to initialize player` — that warning in the dx
  log is expected/benign.
- `i.scdn.co` artwork must never be blocked; `*.spotifycdn.com` ad/preview
  gates are the ones the blocklist targets.
- No direct dioxus signal access from the wry IPC handler — queue and drain.

### 6.6a Ad-block engine v2 (Brave-style `adblock` crate)

- **`adblock::Engine` is `!Send + !Sync`** (uses `Rc`/`RefCell` internally).
  It cannot live in a `static`.  A dedicated std thread owns the engine and
  checks URLs via `mpsc::SyncSender`/`Receiver` channels.  `should_block_url`
  sends the URL and blocks on the reply — safe from any thread including tokio
  workers.
- **Format splitting is required.** The `adblock` crate's `FilterSet` treats
  content as either ABP/uBO Standard (`||domain^`) OR hosts format
  (`0.0.0.0 hostname`), never both in one call.  `split_blocklist_formats()`
  separates them before calling `add_filter_list` with the correct
  `ParseOptions { format }`.
- **`Engine::deserialize` is `&mut self`**, not a static constructor.  Create
  with `Engine::default()`, then call `.deserialize(&bytes)`.
- **`add_filters`/`add_filter` are `#[cfg(test)]` only.** Use
  `add_filter_list(String, ParseOptions)` for production code.
- **Engine cache** at `{cache_dir}/adblock_engine.bin` enables fast restart.
  The engine thread tries deserialization first; falls back to compiling from
  blocklist text on cache miss/corruption.
- **Cosmetic CSS** (`mod.rs::cosmetic::HIDE_UPSELL_CSS`) is gated behind the
  `hide_upsell` setting toggle (disabled by default; ToS-sensitive).  Inject
  into the login/session WebView only when the toggle is ON.
- **`hickory-resolver` is kept** for DNS-over-HTTPS during adblock bootstrap
  (proving the filter doesn't block `api.spotify.com`).  It is NOT used for
  runtime URL blocking — the `adblock` engine handles that entirely.
- **`radix_trie` is removed.** The entire old `dns_filter.rs` module (radix
  trie + reversed-label lookup + `ALWAYS_ALLOW` whitelist) is deleted.  The
  Brave engine subsumes all of it.

### 6.7 Auth specifics

- No `SPOTIFY_CLIENT_ID`, no OAuth scopes, no redirect URIs. The app signs in
  exactly like open.spotify.com: an in-window WebView loads `open.spotify.com`,
  the user logs in (password / 2FA / passkey), and injected JS polls
  `open.spotify.com/get_access_token` (requires the HttpOnly `sp_dc` cookie) to
  capture the web-player access token.
- Session cookies live in the WebView data directory
  (`auth::webview_data_dir()`), which is what keeps the user logged in across
  restarts. The short-lived access token (~1h) is mirrored to the OS keychain
  (`token_store`) so startup can restore a session without opening a window.
- The access token has **no refresh token**; the login WebView stays alive
  (hidden) after sign-in and refreshes it via the same endpoint
  (`spotify/session.rs` → `webview_bridge::request_token_refresh` →
  `webview_login::refresh_token`). See §6.8 for why it must be the login WebView
  and not the SDK WebView (CORS).

### 6.8 The sign-in flow (in-window, open.spotify.com-style auth)

- The sign-in is hosted **inside the main window**, not a separate window. The
  sign-in WebView is packed into the window's existing `vbox` (next to the
  dioxus UI) and the dioxus UI children are **hidden** underneath it; capturing
  the session hides the sign-in widget and re-`show()`s the native UI.
- **Never reparent a realized WebView.** Moving a WebView widget between GTK
  containers (an overlay `add_overlay`, etc.) is what produced the blank white
  screen — webkit's surface does not survive the unmap/remap reliably. The
  sign-in flow only ever `hide()`/`show()`s widgets inside the untouched `vbox`.
- **The login WebView stays alive after sign-in as the session WebView.**
  Because its page IS `open.spotify.com`, it is the ONLY WebView whose
  `get_access_token` fetch works: the hidden SDK WebView is a null-origin page
  and its cross-origin credentialed fetch is CORS-blocked (`TypeError: Load
  failed`), which previously produced "Token refresh timed out" and an
  unloaded Home page. `webview_bridge::request_token_refresh()` evals
  `window._relay.refreshToken()` (defined by `POLL_JS`) in the session WebView
  and falls back to the SDK WebView only when none is alive. It is torn down on
  logout / session-expiry (`webview_login::shutdown`) so the next login starts
  fresh — `start()` refuses to run while a session WebView is alive.
- **Direct `get_access_token` calls are reCAPTCHA-gated** (Spotify tightened
  the endpoint in 2025): a bare fetch now returns a Google invisible-reCAPTCHA
  challenge page, so `r.json()` throws `SyntaxError: The string did not match
  the expected pattern.`. Since Aug 2026 the web player itself fetches from
  **`open.spotify.com/api/token`** instead, whose only challenge is a **TOTP
  computed locally** (RFC 6238 HOTP, SHA-1, 6 digits, 30s period; the key is a
  deobfuscated constant in the `web-player.*.js` bundle — XOR each char with
  `index % 33 + 9`, join the results into a decimal string, use its bytes as
  the HMAC key; `totpVer` is the bundle's version, 61 as of 2026-08). With the
  session cookies + `reason=transport|init&productType=web_player&totp=..&totpServer=..&totpVer=61`
  it returns `{accessToken, accessTokenExpirationTimestampMs, isAnonymous}`.
  `POLL_JS` calls this directly, plus the fetch hook watching the page's own
  `/api/token` traffic, both cached in `window.__spotifyDxToken` and served by
  `refreshToken` (polling before the captcha-gated legacy endpoint). The TOTP
  is computed with a **pure-JS HMAC-SHA1** (embedded, verified against Node and
  Python) rather than `crypto.subtle` — the first attempt used WebCrypto and
  failed silently (a synchronous throw when `crypto.subtle` is unavailable
  left the in-flight guard latched and produced no output). `tryApiToken` is
  fully synchronous, wrapped in try/catch, and aborts hanging fetches.
  The login DOM fallback waits ~10s for these captures before settling on an
  empty token.
- **The `/api/token` TOTP key MUST be the FULL deobfuscated string.** The first
  version truncated it to the first 40 digits (`…8471124`); the correct one is
  the entire 60-digit decimal string
  `376136387538459893883312310911992847112448894410210511297108`. A truncated
  key silently produces wrong TOTPs, which the endpoint rejects with a generic
  `400 {"error":{"code":400,"message":"Unauthorized request","extra":{"_notes":
  "Usage of this endpoint is not permitted under the Spotify Developer Terms …"}}}`
  — indistinguishable from a missing cookie/header. Verified via curl + the
  session cookies: with the correct key the endpoint returns HTTP 200
  `{accessToken, accessTokenExpirationTimestampMs, isAnonymous}` and needs NO
  extra header (a `client-token` from `clienttoken.spotify.com/v1/clienttoken`,
  client id `d8a5ed958d274c2e8ee717e6a4b0971d`, is optional). `isAnonymous` is
  true for guest sessions and false once the cookies identify a logged-in user.
- **The embedded SHA-1 constants in `POLL_JS` are correctness-critical.** The
  second constant MUST be `0x98BADCFE`; a stray `0x98BADCFC` (typo'd in commit
  `70a4ed3`) silently produces wrong SHA-1/HMAC → wrong TOTPs → the endpoint
  returns the same `400 "Unauthorized request" / Developer Terms` error. It is
  NOT exposed by the anonymous tests: the endpoint is lenient about the TOTP for
  guest (no-cookie) requests and only enforces it once session cookies are
  attached, so a curl smoke test without cookies passes while a logged-in app
  fails on every capture. Verify a change with the golden test: run
  `POLL_JS`'s `totpFor()` in Node/src + Python (RFC 6238, key =
  ASCII bytes of the decimal string) and diff the output.
- **`/api/token` debug error mapping.** `400 Unauthorized request` with the
  Developer-Terms note = the server rejected the authenticated TOTP (wrong key
  digest OR wrong SHA-1). `401 Unauthorized` (plain, no note) = invalid/stale
  session cookies on `/api/token`. A `TypeError: Load failed` on the token fetch
  after a nav = the fetch raced a `PageLoadEvent::Started` (page gone to
  about:blank by the idle `park`); on-demand refresh revives the page first.
- **The keychain token is a hint, not the session.** Since the app now always
  shows the `open.spotify.com` WebView at startup (that page is the source of
  truth, and it needs the webview to keep refreshing tokens), `auth::init()` no
  longer writes `AUTH_STATE` or fast-paths past the login gate — it only
  reports whether a clock-valid token exists (used by the headless build). A
  stored token that went stale server-side (e.g. rate-limited to 429 on
  `api.spotify.com`) previously landed the app on a Home screen that spun
  forever.
- **Stop the periodic pollers once the session is captured (idle-CPU fix).**
  The hidden session WebView keeps the full `open.spotify.com` page rendered
  forever, and `POLL_JS`'s original `check()` ran on a fixed `setInterval(check,
  1500)` FOREVER (dropping the interval handle / checking `reported`). Each tick
  fired `tryApiToken()` (2× `/api/token` HTTP + a full pure-JS HMAC/TOTP) **and**
  a legacy `get_access_token` fetch — i.e. ~3 network requests + crypto every
  1.5s, even after login was already captured, keeping the hidden webview busy
  and contributing to the 5-10% idle CPU. `post()` now `clearInterval`s both the
  1.5s `check` timer and the 1s `flushIpc` timer the moment a non-anonymous token
  is reported (and `check()` early-returns once `reported`). On-demand token
  refresh is UNAFFECTED: `_relay.refreshToken()` calls `tryApiToken()` / re-reads
  `window.__spotifyDxToken` itself, and the fetch hook still forwards the page's
  own later captures via `notifyToken` (`token_refresh_result`), which keeps
  `AUTH_STATE` fresh just as before. Do not reintroduce an unconditional polling
  interval here.
- **Late token captures must not be dropped.** After login completes
  (`reported` is latched), `store()` still posts `token_refresh_result` so
  `AUTH_STATE` stays current. **But do NOT make page fetches reactive to
  `AUTH_STATE`.** The session WebView writes a fresh token into `AUTH_STATE`
  every ~2s, so any `use_resource` future that reads it (even transitively via
  `session::ensure_token`) is cancelled and restarted on every capture — a
  page fetch like `get_home()` is restarted every 2s and never completes,
  leaving the page spinning forever. `ensure_token` therefore reads
  `AUTH_STATE` with `.peek()` (non-subscribing), and recovery from failures is
  driven by explicit timers (Home re-fetches the feed 60s after a
  rate-limit error), not by token-refresh reactivity.
- **The bridge IPC queue drops document-start messages.** `webview_bridge`'s
  `IPC_QUEUE` drain task is only spawned by `webview_bridge::init()`, which runs
  AFTER the session WebView has loaded. Anything `POLL_JS` posts at
  document-start (e.g. `token_debug` diagnostics, early `/api/token` results)
  is silently discarded (`IPC_QUEUE.get()` is `None`), while `logged_in` (handled
  locally in `webview_login::handle_ipc`) and late `token_error` arrive. So
  diagnostics from the login/session WebView are logged directly from the
  webkit thread in `webview_login::handle_ipc`, NOT forwarded through the
  bridge queue.
- **Every session needs a session WebView.** The login gate always runs
  (`auth::init()` no longer fast-paths past it), so the session WebView is the
  login WebView kept alive after sign-in. `player::init()` still calls
  `auth::webview_login::ensure_session()`, which builds a hidden, never-shown
  session WebView when none exists (idempotent) so token refreshes always have
  a same-origin WebView to fetch through.
- The session WebView's IPC handler forwards every non-`logged_in` message
  verbatim to `webview_bridge::handle_ipc`, so its `token_refresh_result`
  answers land on the shared `REFRESH_TX`/`IPC_QUEUE` machinery.
- **No `unsafe` is needed.** The whole flow uses only gtk-rs 0.18 safe APIs
  (`children()`, `hide()`, `show()`, `unparent()`, `upcast::<gtk::Widget>()`),
  so the crate stays `#![forbid(unsafe_code)]`-clean. gtk 0.18 is the version
  tao/wry already pull in. `WebViewExtUnix::webview()` returns the
  `webkit2gtk::WebView` widget — store it upcast to `gtk::Widget`.
- **ONE process-wide `WebContext` for all WebViews.** Both the login WebView
  and the hidden SDK WebView are built via `auth::with_session_context`, a
  closure API backed by a `thread_local!` `SESSION_CONTEXT` (created lazily at
  `auth::webview_data_dir()`). This is mandatory: webkitgtk ABORTS when a second
  `WebContext` claims a data directory still held by a live (cached) web
  process — the old two-contexts-on-one-dir login window design was the crash
  behind the "app exits ~7s after login" symptom.
- `with_session_context` takes a closure because the `RefCell` guard cannot
  escape the `thread_local`'s `.with` — build the `WebView` inside the closure
  and return it (a `WebViewBuilder` borrows the context and cannot escape).
- The login IPC handler runs on the webkit thread — it only sends over the
  oneshot channel; the awaiting task on the UI thread writes state and removes
  the WebView (same rule as §6.5).
- **Dioxus signals can only be touched from inside a dioxus runtime.**
  `tokio::spawn` runs on a bare tokio worker and panics (`Must be called from
  inside a Dioxus runtime`). Use `dioxus::prelude::spawn` when the current
  context is already inside the runtime, and NEVER touch signal state from
  `main.rs`'s pre-launch `bootstrap()` (no runtime exists yet) — the very first
  `GlobalSignal::read/write` there panics at `Runtime::current()`. To make
  `ensure_token()` safe outside the runtime (e.g. store SWR background
  refresh), it now checks `Runtime::try_current().is_none()` and returns an
  auth error instead of panicking — the caller serves stale data. This
  actually crashed the app at startup whenever a *valid* token happened to be
  in the keychain: `auth::init()` used to write `AUTH_STATE` directly. Now it
  parks the restored session in a plain `static Mutex` and `App`'s use-effect
  applies it to `AUTH_STATE` on mount (see §6.7 `auth/mod.rs`). `Login` gates
  its auto-start on `auth::pending_restored_session()` so it doesn't open a
  WebView the frame before a restore lands.
- **`open.spotify.com/get_access_token` is unreliable** (Spotify tightened it
  in 2025: 403/400 for many callers, TOTP requirements, changing response
  shapes). The login poller therefore captures the token THREE ways: a
  document-start `fetch` hook that observes the web player's own token request
  (the real player always needs one), a direct poll, and finally a DOM signal
  (profile widget present) that proceeds token-less and lets the hidden SDK
  WebView fetch a token from the shared cookies (`auth::login()` handles the
  empty-token case). Do not "simplify" this back to a single poll.
- **Concurrent token-refresh requests must all be answered.** Two `ensure_token`
  callers can legitimately refresh at once right after login (Home's feed fetch
  + App's `refresh_profile()` backfill). The old single-slot
  `REFRESH_TX: Option<Sender>` silently dropped the first sender when the second
  request overwrote it, which surfaced as a spurious
  `session: token refresh timed out (10s)` on Home even though the refresh
  succeeded. `REFRESH_TX` is now a `Vec` and `apply_token_msg`/the "no webview"
  fast-fail answer EVERY pending sender. `ensure_token` also re-checks
  `AUTH_STATE` after a lost/timed-out answer, because the refresh may still have
  landed via a periodic capture.
- **Never let a token capture poison the stored expiry.** A capture whose JS
  object lacks `accessTokenExpirationTimestampMs` arrives as `expiresMs: 0`;
  blindly storing that turned a perfectly valid ~1h token into "expired", so
  every page entered the 10s refresh path. `apply_token_msg` now ignores
  `expires_ms == 0`.
- **The webview's in-memory session and the on-disk cookie file can diverge.**
  After login, the session WebView captures non-anonymous tokens (`anon=false`)
  while `~/.local/share/spotify-dx/webview_session/cookies` can still yield an
  anonymous `/api/token` response to curl — webkitgtk's cookie flush lags the
  live session. Don't debug login against the cookie file; trust the app's
  `anon=` log lines.
- **`api.spotify.com` 429 rate limits are real and can be fed by the app
  itself.** During an outage window every `/v1/*` call returns
  `429 API rate limit exceeded` regardless of token validity. `App`'s use-effect
  used to re-spawn `refresh_profile()` on every `AUTH_STATE` write (i.e. every
  ~1.5s token capture) whenever `user_id` was still `None` — a failing
  `/v1/me` fetch kept the limit hot. The effect now boots the backend once and
  spawns the profile backfill once (`backend_booted` / `profile_backfilled`
  signals). `get_current_user_profile` still has no 429 backoff (it's best-effort
  and swallows errors); `api_get_json` does.
- **429 handling is fail-fast + self-healing, never an endless spinner.** The
  old Throttled path slept the full `Retry-After` (~40s+) before each retry, so
  Home sat on a spinner for minutes during a rate-limit window and looked
  broken. Now `api_get_json` waits at most `min(Retry-After, 5s)` once, retries
  once, and on a second 429 returns `AppError::RateLimited`. Home detects that
  error, shows a "Spotify's API is temporarily limiting requests" banner, and
  re-fetches the feed on a 60s timer (`retry_count` signal + `use_effect`) until
  the quota clears — at which point the feed loads with no user action.
- **As of 2026-08-17, valid web-player tokens were hard-429'd by
  api.spotify.com for many hours** (every `/v1/*` endpoint, `Retry-After` 11–57s
  that never actually cleared even after a 70s quiet wait). It is NOT
  IP-based (no-token/garbage-token requests get a clean 401), not fixed by the
  `client-token` header the web player sends, and not fixed by browser-like
  Origin/Referer/cookies. **Resolution (2026-08): route all user/library/home/
  browse/search reads through Spotify's internal GraphQL API on
  `api-partner.spotify.com/pathfinder/v2/query` instead of `/v1`** — pathfinder
  accepts our same web-player token, is far less rate-limited, and is what the
  web player itself uses. See `src/spotify/gql.rs` + `docs/RESEARCH.md` §2.5.
  The earlier claim that "pathfinder didn't accept our token (401/404)" was a
  transient hardening/bad-headers artifact, not a hard block.
- `auth::init()` no longer writes `AUTH_STATE` — see §6.7 note above.

### 6.8b Spotify GraphQL data layer (`api-partner.spotify.com/pathfinder`)

- **User/library/home/data reads go through Spotify's internal GraphQL API, not
  `/v1`.** Implemented in `src/spotify/gql.rs`. Endpoint:
  `POST https://api-partner.spotify.com/pathfinder/v2/query` with a
  persisted-query body `{ variables, operationName, extensions: { persistedQuery:
  { version: 1, sha256Hash: "<hex>" } } }`.
- **It accepts the same web-player token** we already capture from
  `open.spotify.com/api/token`. Required headers: `app-platform: WebPlayer` plus
  `Origin`/`Referer: https://open.spotify.com/` (see
  `client::filtered_post_pathfinder`). Without `app-platform`+Origin, pathfinder
  rejects the token.
- **The sha256 hashes are the load-bearing secret and Spotify rotates them.**
  On `412 PersistedQueryNotFound`, refresh the hash (Spotufi pulls a remote
  registry; we keep the current hashes inline in `gql.rs::hashes`). Routed through
  GQL (names + hashes): user playlists (`libraryV3` `973e511c…`), liked songs
  (`fetchLibraryTracks` `087278b2…`), saved albums (`libraryV3` filter=Albums),
  playlist detail + tracks (`fetchPlaylist` `346811f8…`), single-track metadata
  (`searchDesktop` `4801118d…`), album detail + tracks (`getAlbum`/`queryAlbumTracks`
  `b9bfabef…` — NOTE: the server projects a *reduced* response when the request
  `operationName` is `queryAlbumTracks` vs the full metadata+tracks when it is
  `getAlbum`; keep `operationName = "getAlbum"` in `gql_album`), artist page
  (`queryArtistOverview` `ae0e2958…` → hero + discography albums/singles + popular
  tracks in one call), and related artists (`queryArtistRelated` `3d031d6c…`).
  The op-name→hash map is extracted from `open.spotifycdn.com/cdn/build/web-player/web-player.*.js`
  (pattern: `new <x>.l("<OpName>","query","<64-hex>",null)`).
- **`/v1` reads are now fully eliminated** for the app's data views (search,
  library, playlist, album, artist, home). The only remaining `/v1` calls are the
  single, low-volume `get_current_user_profile` (`/v1/me` at login,
  `src/spotify/api.rs`) and `/v1/me/player` playback-control *writes*
  (`src/spotify/player_api.rs`, user-initiated). Everything else goes through
  pathfinder. The old `/v1` GET pipeline (`cached_get_json`, `pipeline_load`,
  `request_once`, `request_after_backoff`, `classify`, `ResponseOutcome`,
  `get_object`, `get_featured_playlists`, `get_new_releases`,
  `get_recommendations`, `get_artist*`) was removed as dead.
- **Playlist track counts from `libraryV3` are unreliable** — the item carries a
  count only under a few schema-dependent keys (`trackCount` /
  `content.totalCount` / `totalLength`), often 0 for library playlists. The home
  shelf hides "0 tracks" and shows just "Playlist" when the count is unknown;
  the playlist detail page gets the real count from `fetchPlaylist`.
- **GQL response shapes** differ from `/v1`: track artists live at
  `artists.items[].profile.name`, album at `albumOfTrack`, covers at
  `coverArt.sources[]` / playlist `images.items[].sources[]`, track URIs may sit
  on a wrapper `_uri` instead of `data.uri`. Liked songs parse from
  `data.me.library.tracks.items[].track.data`. Playlist tracks parse from
  `data.playlistV2.content.items[].itemV2.data`.
- **Playback must not call `/v1` for metadata.** `player::launch_track(Track)`
  plays using metadata already in hand (the UI has the full `Track` object the
  user clicked), so rows/cards call `launch_track` and skip the network. The
  URI-only fallback (`player::launch(uri)`) resolves via GQL `searchDesktop`.
  Only legacy/premium paths touch `/v1/player`.

### 6.9 Open streaming engine (Phase 4b)

- **`rodio 0.22` API differs from newer versions.** Uses `MixerDeviceSink`
  (from `rodio::stream`), `rodio::play(&Mixer, reader)` returns a `Player`,
  and `DeviceSinkBuilder::open_default_sink()` for device init. NOT `Sink`,
  `OutputStream`, or `Sink::try_new`.
- **`MixerDeviceSink` is `!Send`.** The audio sink runs on a dedicated std
  thread that owns the device; commands arrive via `mpsc::channel`.
- **`SETTINGS` access from non-dioxus threads** requires
  `use dioxus::prelude::ReadableExt;` to bring `.peek()` into scope.
  From a dioxus task, `.read()` subscribes; from a bare thread, use `.peek()`.
- **Provider resolution is async.** The `resolver::resolve()` function and
  each provider's `resolve()` are async. The sink thread calls `resolve()`
  via `tokio::spawn` and waits for the result.
- **TIDAL uptime list** (`tidal-uptime.geeked.wtf`) is fetched on first
  access and cached for 5 min. Falls back to hardcoded instances.
- **Stream URLs expire** (~1h). The URL cache uses a conservative 50-min
  TTL.
- **`async-trait` is required** for the `Provider` trait because its
  methods are async and the trait is used as `dyn Provider`.
- **`PLAYER_STATE.volume` starts at 0.0** (derive `Default`) and was NOT seeded
  from `settings.json` — the open engine set the audio sink to volume 0 and was
  completely silent despite decoding fine. Fix (kept): `App` seeds
  `PLAYER_STATE.volume` from `SETTINGS.volume` exactly once at mount via
  `player::seed_volume_from_settings()`. If the audio is ever silent again,
  first check this, then the rodio `Player::connect_new(mixer)` → `append(...)`
  wiring (decode itself is verified audibly correct).
- **`TrackRow`/`track-table-head` grid columns must match the index presence.**
  The row grid is `30px 50px minmax(0,1fr) auto`. When `numbered: false` (e.g.
  home "Liked songs") there is NO index span, so the art/title/duration children
  auto-place one column left (art→30px, title→50px, duration→1fr) and everything
  looks crammed to the left. Rows use the extra class `track-row--noindex`
  (`50px 1fr auto`) and the header `track-table-head--noindex` (same) when the
  index is absent; the header always emits an art-column spacer so labels align.
- **`get_user_albums` used to hit `/v1/me/albums` → 429** → the whole Library
  page showed "Couldn't load your library" (because the page treats ANY one of
  the three parallel calls erroring as a hard failure). Migrated to the GQL
  `libraryV3` operation with `filters: ["Albums"]` (same pattern as
  `get_user_playlists`, which uses `filters: ["Playlists"]`). Album items use
  `item._uri` on the wrapper + `data.coverArt.sources[]` / `artists.items[]`.
  Lesson: keep ALL `/v1` reads off — any library/browse/page call must go
  through pathfinder or it will 429 after a handful of requests.
- **Search hit `/v1/search` → 429** → the "old design" / red
  "rate-limiting…retrying" banner. Migrated `api::search` (and thus
  `search_tracks`) to the GQL `searchDesktop` operation. Result nodes:
  `searchV2.tracksV2.items[].item.data` (Track), `searchV2.albumsV2.items[].data`
  (Album), `searchV2.artists.items[].data` (Artist, images at
  `visuals.avatarImage.sources[]`). Albums in search expose `date` as
  `{year}` (NOT `isoString`), so `album_release_date` tolerates both; artist
  genre/followers aren't present and are left defaulted. `live_get_json` was
  search's only caller and is now removed.
- **GQL track duration field differs by operation.** `libraryV3`/`fetchLibraryTracks`
  expose duration as `duration.totalMilliseconds`; `fetchPlaylist` item tracks use
  `trackDuration.totalMilliseconds`. `parse_gql_track` checks both (plus
  `durationMs`/`duration_ms`). Missing the playlist variant showed "0:00" for every
  playlist track while liked tracks (which parse `.duration`) were correct.
- **Odesli API is dead.** `api.song.link` now returns
  `401 PUBLIC_API_ACCESS_DEPRECATED` — Linktree officially sunset the public
  Odesli API; it now requires a paid API key (email `developers@song.link`).
  Consequently the **TIDAL and Qobuz providers are disabled**: they depended on
  Odesli for Spotify→platform ID mapping (their own search APIs need paid auth,
  and the community proxies return 404). Both now report `is_available() ==
  false` so the resolver skips them without the futile `odesli::resolve()` call.
  Re-enable only if a working ID mapper appears (flip `is_available()` to true).
- **YouTube provider is the sole active source** and is fully self-contained (no
  Odesli). It uses InnerTube with the ANDROID client (`clientVersion
  20.10.38`, `androidSdkVersion 30`, plus `osName: Android`/`osVersion: 11`
  — these are REQUIRED or YouTube returns 400 `FAILED_PRECONDITION`) and the
  ANDROID API key `AIzaSyA8eiZmM1FaDVjRy-df2KTyQ_vz_yYM39w`. The ANDROID
  client returns direct (non-signature) audio URLs, so no JS/PO-token
  handling is needed. Search parses
  `contents.sectionListRenderer.contents[].itemSectionRenderer.contents[]`
  using `compactVideoRenderer` (fall back to `videoRenderer`). Stale client
  versions (e.g. `2.20240101.00.00`) are rejected — keep `CLIENT_VERSION`
  current (ref: yt-dlp `INNERTUBE_CLIENTS`).
- **YouTube throttles the adaptive (audio-only) formats — use the muxed
  format instead.** The `adaptiveFormats` URLs carry `gir=yes`; they are
  IP-bound and throttled: a sustained download 403s after ~1MB regardless of
  range size, pacing, or fresh-URL rotation (measured hard cap = 1,000,000
  cumulative bytes per IP). A full 3-4MB song can NOT be fetched that way.
  The progressive muxed format (`streamingData.formats`, itag 18 = 360p mp4
  with an AAC audio track) has NO such restriction: a plain GET returns the
  entire file with any User-Agent. The YouTube provider therefore selects the
  muxed format first (`AudioFormat::Aac`), falling back to adaptive formats.
  rodio needs the `mp4` feature to decode it (see Cargo.toml — the app only
  enables `playback` + `mp4` on rodio).
- **All full-audio lossless/other sources are dead or locked down (2026):**
  TIDAL/Qobuz (Odesli sunset), Invidious/Piped anonymous API instances (401/
  403 from this IP). YouTube-muxed is currently the only reliable full-track
  path. If ever blocked, the fallback is Spotify 30s previews via
  `p.scdn.co/mp3-preview` using the captured web session.

### 6.9a Idle CPU & timer hygiene (learned fighting a 5-10% idle / 20% startup spike)

The app was burning ~4-10% CPU fully idle. Culprits and the fixes (keep these
patterns in mind so new code doesn't reintroduce them):

- **Never write a `GlobalSignal` on a timer unless a value actually changed.**
  An unconditional `PLAYER_STATE.write()` every 250ms (the open-engine position
  poller in `player::open_play_track`) and an unconditional `ADBLOCK_STATS`
  write every 1s (`ui::components::app_layout`) each marked the signal dirty on
  every tick, re-rendering `PlayerBar`/`NowPlayingView`/SideNav even with
  nothing changing. Fix: `peek()` the current value first and only acquire the
  write lock when a field really differs (dioxus re-renders a signal's
  subscribers when the write guard is dropped, so merely taking a write lock to
  "assign the same value" still triggers a render). `AdblockStats` and
  `PlayerState` are `Copy`/`PartialEq`-derived so `*ADBLOCK_STATS.peek() != new`
  is a cheap gate.
- **Prefer event-driven over polling — a shared ticker should await a signal,
  not `interval()`.** `open_play_track` used to `spawn` a fresh 250ms poller
  every time a track played, never cancelling the old ones (N tracks → N
  concurrent pollers fighting over `PLAYER_STATE`). The position sync is now a
  single process-wide task started once via a `static POSITION_TICKER:
  OnceLock<()>` and reused for every track. It no longer `interval(250ms)`s:
  `media::sink::SinkState` carries a `position_changed: tokio::sync::Notify`,
  and the ticker `await`s `position_changed.notified()` — the sink only notifies
  while it is actually playing/publishing, so once playback stops the task
  sleeps forever (zero wakeups). It reads state via the non-spawning
  `sink_state()` getter (never forcing the audio thread into existence) and
  still does peek-then-write. `Notify::const_new()` lets a `static Notify` live
  alongside a `Lazy` RwLock.
- **Don't `peek()`/`write()` a dioxus `GlobalSignal` from an event-driven task
  with the panicking API — use `try_write_unchecked()`.**
  `GlobalSignal`s (`PLAYER_STATE` etc.) are backed by **thread-local ("unsync")
  storage**, and `dioxus::prelude::spawn` tasks run on the **single UI-thread
  executor**. If another UI task transiently holds the write lock across an
  `.await`, the executor can poll your task mid-borrow and its `peek()`/`write()`
  call panics with `AlreadyBorrowed` — a hard crash we hit on first playback. The
  event-driven position ticker now takes the whole read-and-write through ONE
  `PLAYER_STATE.try_write_unchecked()` (needs `use dioxus::prelude::Writable;`),
  and on `Err` simply `continue`s to the next sink publish (≤250ms later), so it
  can never abort. Also: never read+write the same signal in one expression
  (`PLAYER_STATE.write().x = PLAYER_STATE.peek().y...`) — the write guard lives
  for the whole statement, so the RHS `peek()` re-borrows and panics; hoist the
  read into a local first.
- **A dedicated OS thread should block (not poll) when it has no work.**
  `media::sink` ran `recv_timeout(250ms)` forever; it now branches on whether a
  player is active — while idle (`current_player == None`) it calls the blocking
  `recv()` so it sleeps with zero wakeups, and only uses `recv_timeout(250ms)`
  for position publishing while something is playing (and it is this publish
  that `notify_waiters()`es `position_changed` for the ticker above).
- **Recheck-loops should sleep longer when idle, or be gated off entirely.**
  The `PlayerBar` clock coroutine used to `interval(250ms)` forever; it now only
  runs on the SDK path at all — it is gated on `!is_open_engine()` (the free
  account uses the open/YouTube engine, so there is no SDK clock to fake-advance
  and the coroutine body is dead code there). While on the SDK path it
  busy-ticks at 250ms only when advancing the clock, else `sleep(500ms)`.
- **Mirror stats with a `Notify`, not a 1s timer.** `app_layout` used to write
  `ADBLOCK_STATS` every 1s (compare-then-write since v4). It now `await`s
  `adblock::stats_changed()`, a `static STATS_CHANGED: Notify` fired by
  `adguard_api::record_drop()` and after a blocklist refresh — so the task
  sleeps forever while no ads are being blocked. An inner re-read loop coalesces
  bursts into a single render. (The `ADBLOCK_STATS.read()` in `nav.rs` still
  subscribes the component, but with event-driven writes there are no periodic
  re-renders.)
- **The hidden session WebView's injected JS pollers must stop after login.**
  `POLL_JS` in `auth::webview_login` used a fixed `setInterval(check, 1500)`
  forever (≈3 fetch()s to `/api/token` + a pure-JS HMAC/TOTP per tick, even
  after the session was captured). `post()` now `clearInterval`s the `check` and
  `flushIpc` timers the moment a non-anonymous token is reported, and `check()`
  early-returns once `reported`. On-demand `_relay.refreshToken()` and the page
  fetch-hook forwards are unaffected.
- **Park the session WebView at `about:blank` when idle (the biggest idle-CPU
  win).** Even with our JS pollers halted, the session WebView was keeping the
  whole `open.spotify.com` SPA rendered offscreen
  forever just to be a same-origin token-refresh channel. `hide()` (login
  capture) and `ensure_session()` (safety net) now navigate it to `about:blank`
  (a per-instance `ready: Arc<AtomicBool>` + `suspended: bool` on the
  `LoginWebView`; `ready` is driven by the `PageLoadEvent::Finished` handler and
  cleared on `Started`). `refresh_token()` is now `async`: if `suspended`, it
  revives the page (`load_url(SPOTIFY_LOGIN_URL)`) and waits (polling
  `ready`, ≤6s) for the page + `POLL_JS` to finish loading before eval'ing
  `_relay.refreshToken()`. `webview_bridge::request_token_refresh()` is `async`
  and awaits the revive WITHOUT holding `REFRESH_TX` (clippy
  `await_holding_lock` — a 6s lock across await would block all other
  refreshes/drains). `session::ensure_token` awaits it. Revival only happens on
  real token expiry (once per ~hour), so the visible cost (a page reload) is
  negligible and absorbed by the existing 10s timeout. NOTE: parking is safe
  for restored sessions too — `AUTH_STATE` already holds the captured token
  after `login()`/`hide()`, so first page loads don't revive; and the visible
  `start()` WebView (not `ensure_session`, which is a no-op once `start()` has
  run) is what flips `is_authenticated` on both fresh and restored launches.
- **No `infinite` CSS animations on ever-present elements.** The sidebar's
  `.nav-ready::before` "blocker active" dot ran
  `animation: pulse 2.2s ease-in-out infinite`, keeping WebKitGTK's compositor
  repainting the main WebView forever even with a fully static page — a real,
  continuous idle-CPU cost that browsers avoid (their idle tabs stop running
  decorative animation timelines). Changed to a one-shot `pulse 2.2s ease-in-out`
  (single fade-in on mount; dot stays lit after). Loading-only animations
  (`.spinner` 0.75s spin, `.skeleton` 1.4s shimmer) are fine because they unmount
  once content loads — the rule is: an animation that renders at idle must not
  loop. When auditing idle CPU in a WebView, grep `assets/main.css` for
  `animation:.*infinite` as a first pass.

### 6.9b Mobile/platform parity (the `native` seam + GTK-coupled webviews)

- **`dioxus::mobile` is literally a re-export of `dioxus::desktop`** (both wrap
  wry/tao; `dioxus-0.7.10/src/lib.rs` does `pub use dioxus_desktop as mobile`).
  So   native renderers (desktop + mobile) share the wry stack; only WASM differs.
  The feature graph now has a shared `native = ["dep:wry"]` enabled by both
  `desktop` and `mobile`; gate truly platform-agnostic native code (media sink,
  adblock, `playback_sdk` HTML bootstrap) on `#[cfg(feature = "native")]`.
- **The SDK webview is now `native`-gated, not desktop-gated** (Phase C landed):
  `player/webview_bridge.rs` and the `playback_sdk` module now compile for mobile
  too, so mobile plays via the SDK like desktop instead of the old Connect-API
  shell. `main.rs` mobile `main` runs `rt.block_on(bootstrap())` then
  `dioxus::launch(App)`.
- **The session/sign-in webview is `native` too**, so `open.spotify.com` opens
  INSIDE the app on mobile, exactly as on desktop: `auth/webview_login.rs` is
  shared, with two hosts — GTK-packed (`build_gtk`) on Linux desktop vs wry's
  cross-platform `build(&window)` (fills the window in-app) on mobile /
  non-Linux desktop (iOS = WKWebView, Android = AndroidView). `auth::login()`
  and `ensure_session()` now run for every native renderer, so mobile gets the
  same in-app login + same-origin token-refresh session WebView as desktop.
  Mobile anti-pattern to avoid: the builder borrows `&mut WebContext`, so the
  build must happen inside `with_session_context` (see `build_in_context`) —
  you can never return a `WebViewBuilder` out of that closure.
- **Pure-mobile build command:** `cargo check --no-default-features --features
  mobile`. `cargo check --features mobile` still pulls the default `desktop`
  feature, so it exercises desktop+mobile together and **masks** renderer-only
  problems — always use `--no-default-features` to test mobile meaningfully.
- **Shared native code cannot name `dioxus::desktop`** (that module only exists
  when the `dioxus/desktop` feature is on, else `dioxus::mobile` re-export is
  used). `src/platform/webview.rs` picks the right alias with `#[cfg(feature =
  "mobile")]` vs `#[cfg(all(desktop, not(mobile)))]` so webview_bridge builds on
  every native platform; both features can be on at once (`--features mobile`).
- **Android cross-build works with a plain `.cargo/config.toml`** — no
  `cargo-ndk` needed. `cargo-ndk` isn't installed here; instead `.cargo/
  config.toml` sets `linker`/`ar` for `aarch64-linux-android`, plus two
  **un-versioned symlinks** (`aarch64-linux-android-clang` →
  `aarch64-linux-android21-clang`, `aarch64-linux-android-ar` → `llvm-ar`) created
  in the NDK bin dir — `cc-rs` resolves those exact names and won't accept the
  versioned ones. Command: `cargo check --no-default-features --features mobile
  --target aarch64-linux-android` (pass with NDK bin on `$PATH`); verified clean
   here on NDK `25.2.9519653`. iOS targets aren't rustup-installed so iOS cannot
   build on this Linux host.
- **Local Android link gotcha: `ld: unable to find library -laudio`.**
   `dx build --platform android` wires its own linker (`-C linker=<dx bin>`),
   which searches the **un-versioned** NDK lib dir
   `sysroot/usr/lib/aarch64-linux-android/` (NOT the API-versioned `28/` subdir),
   and that dir ships no `libaudio.so`/`libaaudio.so` stub. A crate links the
   AAudio API via `-laudio` (resolves to `libaudio.so`), which is missing there →
   the whole link fails. This does NOT affect CI's `dx build --platform android`
   (runs before Gradle in `.github/workflows/release.yml`), so an APK builds fine
   remotely. **Local workaround** (idempotent, edit the host NDK install):
   ```
   SYSROOT=$(ANDROID_NDK_HOME)/toolchains/llvm/prebuilt/linux-x86_64/sysroot/usr/lib/aarch64-linux-android
   ln -sf  28/libaaudio.so  "$SYSROOT/libaaudio.so"   # real file lives in 28/
   ln -sf  28/libaaudio.so  "$SYSROOT/libaudio.so"    # resolves `-laudio`
   ```
   (Do the same under `$SYSROOT/28/` if needed.) `-laudio` then resolves in both
   the versioned and un-versioned search dirs. After the workaround, the full
   local build is: `dx build --platform android --release --target
   aarch64-linux-android` (compile+link `libmain.so`), `scripts/stage-updater.sh`,
   then Gradle `assembleRelease -x lintVital*`, then `apksigner` with a generated
   debug keystore. `CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER`/`NDK_HOME`
   env vars are nice-to-have but `dx` overrides the linker to itself regardless —
   the symlink is the load-bearing fix.
- **Android wry has NO multi-webview layering (the blank-white-screen bug).**
  In wry 0.53.5 the Android backend is single-view: every
  `WebViewBuilder::build(&window)` calls `Activity.setContentView(webview)`
  (see `crates-android_main_pipe`, msg `CreateWebView`), silently detaching the
  previous content, and `set_visible`/`set_bounds` on Android are documented
  no-ops (`android/mod.rs`). So the "stack webviews in one window" model from
  GNOME/iOS is impossible — after the sign-in page was parked at
  `about:blank` the app was stuck on a full-screen white page. Fix lives in
  `src/platform/android_views.rs` (Android-only): [`capture_base`] stashes a
  JNI `GlobalRef` of the pre-existing dioxus UI through wry's `dispatch`, and
  every extra webview installs itself via `on_webview_created` +
  `install_overlay` (re-`setContentView(ui)`, then `addView(webview)` on
  `android.R.id.content` = `0x01020002`). Hiding on Android =
   `setVisibility(GONE)`; detaching = `removeView` — never wry's `set_visible`/
   `set_bounds`. Because of this, the login/session + SDK webviews must call
   `android_views::capture_base()` BEFORE any of their own `build` runs.
   **`setVisibility(GONE)` alone is NOT enough** — verified on a Motorola
   (dubai_ge, Android 14): even GONE, the parked session WebView still composited
   a full-screen white `about:blank` over the real dioxus UI, so the app reached
   `Home` (logs show `home: fetching feed`) yet the screen stayed blank white.
   `webview_login::hide()` now calls `android_views::remove()` (removeView +
   GONE) to fully DETACH the session overlay on Android instead of
   `set_visible(false)`. The `GlobalRef` is passed as a `clone()` and kept alive
   in `login.overlay`, so `refresh_token()` (wry `load_url`/`evaluate_script`)
   still works — neither needs the Android view attached. It also logs an
   `info!` when it detaches and a `warn!` if `login.overlay` is `None` (hook
   didn't populate it), so post-login device logcat tells you whether the
   overlay ref existed. Root-cause evidence: `uiautomator dump` showed exactly
   two full-screen `android.webkit.WebView` nodes `[0,60][2295,1020]` (base +
   one overlay) in the content frame, and the screenshot was pure white content
   `(255,255,255)` with a grey status bar — the parked overlay on top. Only the
   open-engine path exercises this single-overlay case (free accounts); the SDK
   path adds a second overlay via `webview_bridge` (`ensure_sdk_webview`).
   `jni = "0.21"` was added (Android-only dep) purely to type the
   `on_webview_created` closure's `Result<_, jni::errors::Error>`.
 - **Detaching the overlay is necessary but not sufficient — building the
   overlay already broke the base view, permanently.** Root cause, verified by
   code inspection after on-device logs ruled out auth (fresh `force-stop` +
   launch, Motorola dubai_ge Android 14: `session captured (token)`,
   `anon=false`, overlay detached, yet `Home` mounts — `home: fetching feed
   (attempt 0)` — while pixels stay frozen on the unstyled `Login` gate with a
   single `android.webkit.WebView` node in the dump and `Uncaught (in promise)
   NetworkError: ... Failed to load 'https://dioxus.index.html//__events'`
   ~10–20ms after every Router mount): wry 0.53.5's Android backend keeps the
   custom-protocol/IPC/navigation handlers in process-global locks, and EVERY
   `WebViewBuilder::build()` unconditionally `replace()`s them with that
   builder's own (empty, for our login overlay) registrations
   (`wry-0.53.5/src/android/mod.rs`, `REQUEST_HANDLER`/`IPC`/`URL_LOADING_OVERRIDE`
   populated from per-build `custom_protocols`; dispatched per request in
   `src/android/binding.rs::handle_request`). The dioxus base view registers
   the async `dioxus` protocol serving `https://dioxus.index.html/` assets +
   `//__events` (`dioxus-desktop-0.7.10/src/webview.rs:394`,
   `src/protocol.rs:62`); our overlay build wipes it process-wide, so from
   that moment the base view's CSS and event XHRs go to the real network and
   die (unstyled Login, dead taps). View surgery cannot heal this: `removeView`
   leaves it broken, and `WebView.reload()` is fatal — the custom-scheme
   document is served at initial-load time only, so reload issues a real fetch
   dying with `ERR_NAME_NOT_RESOLVED` (verified on-device: white "Webpage not
   available" page; do not retry). `POLL_JS` early-returns on non-`http(s)`
   pages so a parked `about:blank` session page stops re-arming pollers
   (refresh still revives the real page). Resolution (landed): the Android login/session
   page is a platform `android.webkit.WebView`, not wry —
   `src/platform/android_webview.rs` (Android only) drives it entirely over JNI
   through wry's `dispatch` (creation, `loadUrl`, `evaluateJavascript`,
   `getProgress`, `getTitle`, `CookieManager` setup), so wry's globals are
   never touched and the dioxus base view is never reparented, keeping its
   protocol handler intact underneath the overlay. Session cookies interoperate
   via the process `CookieManager` singleton; Rust<->page IPC goes over
   `document.title` (`NATIVE_IPC_SHIM` collects into `window.__spotifyDxOutbox`,
   Rust ferries it out with an eval + `getTitle()` and resets — the outbox var
   is the source of truth, so a clobbered ferry is retried, never lost).
   Drivers in `auth::webview_login` (`drive_login` / `drive_hidden` /
   `native_refresh_token`) re-inject shim + `POLL_JS` on every pass (idempotent
   latches; survives Spotify's accounts→open redirects) and feed results into
   the unchanged    `handle_json`/bridge machinery by rebuilding the equivalent
   `wry::http::Request`. wry overlay code (`android_views::install_overlay`
   etc.) stays for the hidden premium SDK view only. Popup containment: the
   bundle starts with `NATIVE_POPUP_FIX` (rewrites `target=_blank` to `_self`,
   shims `window.open` to same-window, MutationObserver for late DOM) plus a
   default `WebChromeClient` so uncaptured new-window requests fail closed
   in-view instead of escaping to the browser (from where App Links fire the
   real Spotify app); a `getUrl()` guard sends the view back to sign-in on
   `intent:`/`spotify:` URLs. Caveat: premium accounts
   still build that wry SDK view post-login (same clobber) — it needs the same
   native treatment as follow-up; free/auto accounts never build it (verified
   single-`WebView` dump). App-handoff containment (verified via
   `ActivityTaskManager` START lines: our uid firing BROWSABLE
   `https://open.spotify.com/...` explicitly targeted at SpotifyMainActivity
   ~2s after cold start): Spotify's mobile pages auto-fire an app handoff that
   resolves through verified App Links to the real Spotify app — foregrounding
   it on every cold start and on hidden token-refresh revives. The platform
   view therefore uses a desktop Chrome UA (+ wide viewport / overview mode)
   whose site variant has no such handoff, starts directly at
   `open.spotify.com` instead of the accounts bounce page, and keeps the popup
   neutralizer + `getUrl()` guard as backstops. Device forensics (cookie DB
   holds live `sp_dc`/`sp_key`; authed `curl` of the accounts page returns a
   clean 200 with plain same-window SSO links; the fired intent's datum is an
   `open.spotify.com/?flow_ctx=` continuation) show the handoff is the
   session bounce: with cookies, the accounts page auto-continues and the
   landing triggers the app handoff faster than any post-load injection can
   cover. Android therefore starts at `open.spotify.com` itself (no bounce
   exists there: logged-in boots straight to capture, logged-out gets the
   wall), and the driver logs every in-view URL (`session page at …`). The neutralizer also carries a
   capture-phase click guard (converts `_blank`/non-web-scheme anchor
   activations in-view — this covers programmatically created + synchronously
   clicked anchors, which run inside one JS task and beat the
   `MutationObserver`), reports every interception as `token_debug`
   (`popup-shim-open:` / `popup-guard-blank:` / `popup-guard-scheme:`) so the
   exact escape vector is visible in logcat, and the bundle is (re)injected
   from view creation on (not just at progress 100) to shrink the
   document-start race — plus true document-start registration where
   available: AndroidX `WebViewCompat.addDocumentStartJavaScript` runs before
   the first page script on every navigation (the only mechanism that can
   neuter parse-time auto-handoffs); on this device's packaged AndroidX the
   method is ABSENT (verified tombstone: `NoSuchMethodError`, plus dex
   inspection), so the code treats it as best-effort with post-load inject as
   the working mechanism. Two hard JNI rules learned shipping this (both
   SIGABRT-class if violated): app classes (`androidx.*`) must load through
   `activity.getAppClass` (wry's own pattern) — plain `find_class` uses the
   boot loader, leaves a pending `NoClassDefFoundError`, and aborts the
   process at the next `FindClass` (verified tombstones) — and every JNI
   error path must `exception_clear()` IMMEDIATELY, before any further JNI
   call in the same closure (not just at the end: the abort fires at the next
   internal `FindClass`, so a warn-and-continue without clearing still
   crashes). Applies in `dispatch_call` and all fire-and-forget closures. The view is created with
   explicit `FrameLayout.LayoutParams(MATCH_PARENT)`, focusable /
   focusable-in-touch-mode / clickable / enabled + `requestFocus()`, and touch
   is verified working on-device (tapping the accounts email field focuses it
   and summons the keyboard, `mInputShown=true`). Earlier dead-touch readings
   came from the desktop-UA cookie wall (scroll-locked modal + overview-mode
   coordinate doubt), not the view — the explicit setup stays as cheap
   insurance.
   The view starts at the accounts sign-in page (direct login form, no cookie
   wall) with the default mobile UA (a desktop UA was tried and reverted:
   awkward phone dimensions, and it did not stop the handoff anyway), and the
   driver logs every in-view URL change (`session page at …`) so the page flow
   is traceable without Java-side navigation callbacks. Device forensics
   (cookie DB holds live `sp_dc`/`sp_key`; authed `curl` of the accounts page
   returns a clean 200 with plain same-window SSO links; the fired intent's
   datum is always an `open.spotify.com/?flow_ctx=` continuation URL) show the
   handoff is the session bounce: with cookies present, the accounts page
   auto-continues and the landing page fires the app handoff within ~1s —
   faster than any post-load injection can cover, which is why this stays
   under investigation even with all containment above in place.
- **`&mut JNIEnv<'a>` is invariant over `'a`** — never write a JNI helper that
  joins a `JNIEnv` arg to a `&JObject`/return whose `'local` must unify equal
  (wry `on_webview_created`'s `Context` does share one frame lifetime, but
  `dispatch`'s `|&mut JNIEnv, &JObject, &JObject|` gives each arg an
  independent one). `content_frame!` is a macro for this reason: `call_method`
  infers its own `'other_local`, so inlining JNI calls (rather than returning
  frame `JObject`s from helper fns) sidesteps the whole problem.

### 6.9c Web (WASM) parity seams — storage, audio, adblock, login

- **wasm build command:** `cargo check --no-default-features --features web --target
  wasm32-unknown-unknown` (must exclude the default `desktop` feature, which pulls
  dioxus-desktop→tungstenite→native-tls→openssl-sys). Desktop/mobile/native are
  the default- and native-`feature` combinations.
- **reqwest has no `wasm` feature** — its fetch backend is picked automatically
  from the target arch. Nothing in `Cargo.toml` says `features = ["wasm"]`.
- **Native-only client config** (`cookie_store`, `gzip`/`brotli`, `timeout`) is
  gated `#[cfg(not(target_arch = "wasm32"))]`; the shared HTTP type stays
  `reqwest::Response`.
- **`#[async_trait(?Send)]`** (not plain `async_trait`) for the streaming
  `Provider` trait so wasm's `!Send` futures are allowed.
- **The `src/platform/` seam** is where native/desktop vs wasm diverge: `storage`
  (fs vs localStorage), `spawn_background` (tokio::spawn vs spawn_local), and
  `web_login`. Token store, settings, image/media/stream caches, and store
  snapshots all route through `platform::storage`.
- **Web login = whole-tab redirect.** The browser can't host the GTK login
  WebView, so `auth::login()` on wasm redirects to `open.spotify.com` and then
  fetches `get_access_token` with `credentials: include` (like the desktop
  `fetchAccessToken`). **This credentialed cross-origin fetch is CONFIRMED
  BLOCKED in a real browser (2026-09):** the deployed site on `github.io` gets
  `Failed to fetch` + CORS + 429. Root causes (§6.9g): Spotify sends no
  `Access-Control-Allow-Origin` for `/get_access_token` or `/api/token`, the
  `sp_dc` session cookie is HttpOnly (invisible even to Spotify-tab JS), and
  two tabs on different origins have no shared channel. Web login therefore
  CANNOT work client-side on static hosting; a future backend proxy is planned
  (see §6.9g). Do not claim web login "works" in the browser.
- **build wasm via `web-sys` fetch, not reqwest**, for the login capture so the
  `credentials: include` request mode is explicitly controllable (reqwest's wasm
  fetch backend doesn't expose that the same way).

### 6.9d Android APK packaging — NO Android Studio, NO committed `android/` scaffold

- **`dx build --platform android --release` auto-generates the entire Gradle
  project** from a built-in template in the CLI (`assets/android/gen/` in
  `dioxus-cli-<v>`: `settings.gradle`, root + app `build.gradle.kts`,
  `AndroidManifest.xml`, `MainActivity`, mipmap icons). **There is no `android/`
  directory to commit to this repo, and Android Studio is not required.**
  Earlier assumption that we must recreate a dioxus-mobile scaffold in-repo was
  WRONG — the CLI ships it. Verified in `dioxus-cli-0.6.2` source + the 0.7
  mobile/bundle docs.
- **`dx` only needs env vars** (no GUI): `ANDROID_NDK_HOME`/`NDK_HOME` + SDK as
  `ANDROID_SDK_ROOT`/`ANDROID_SDK`/`ANDROID_HOME`, `JAVA_HOME` (a plain JDK 17;
  Studio's JBR unneeded), and the rustup android target. Resolution order
  confirmed in `dioxus_crate.rs:android_ndk/android_sdk` and
  `cli/target.rs:152` (JAVA_HOME wins).
- **APK output:** `dx build --platform android --release` writes the Gradle
  project to `target/dx/<crate>/release/android/app/`, and the APK (a debug
  build by default) to
  `…/app/app/build/outputs/apk/{debug,release}/`. **The old `target/android/release/`
  path the docs claim is wrong** for the 0.7.10 CLI — verified in
  `packages/cli/src/build/android.rs` (`debug_apk_path`/`release_apk_path`).
- **`dx build` runs `assembleDebug` unless `[bundle.android]` (jks) is set:**
  in `assemble_android()`, the Gradle task is `assembleRelease` ONLY when
  `release && config.bundle.android.is_some()`, else `assembleDebug`. With no
  jks config, `dx build --platform android --release` still emits a *debug*
  APK; producing a release APK requires running `./gradlew assembleRelease`
  yourself (`-x lintVitalAnalyzeRelease -x lintVitalRelease
  -x lintVitalReportRelease` to dodge the AGP 8.7 lint crash, Dioxus#5251),
  which yields an *unsigned* `app-release-unsigned.apk` (no signingConfig) —
  sign it with `apksigner` + a generated keystore.
- **CI arch gotcha:** the default android triple follows the **host** arch
  (`x86_64-linux-android` on x86_64 CI runners). Pass `--target
  aarch64-linux-android` explicitly (or probe adb) or you silently get an
  x86_64 APK. (Dioxus issue #4642 / comment `dx build --android --release
  --target aarch64-linux-android`.)
- **min_sdk_version must be ≥ 30** (Android 11): dioxus/tao call
  `WindowManagerImpl.getCurrentWindowMetrics()` which only exists on API 30+;
  older devices crash with `NoSuchMethodError`. Set it in `Dioxus.toml`
  (`[mobile] min_sdk_version`); repo now has 30.
- **CI (`.github/workflows/release.yml` `android-apk` + `web` jobs, Phase D):**
  - cargo config files do **NOT** expand env vars, so CI overrides the
    machine-specific `.cargo/config.toml` NDK paths with the higher-precedence
    `CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER`/`_AR` env vars — no file edit.
  - CI must re-create the un-versioned `aarch64-linux-android-clang`/`-ar`
    symlinks in the NDK bin dir (`cc-rs` needs the exact names). Point the
    `-clang` one at a **versioned** wrapper ≥ 26 — the un-versioned wrapper
    defaults the API level to 21, whose sysroot has no `libaaudio.so`, and
    `cpal` links `-laaudio` → `ld: unable to find library -laudio`. Prefer
    `aarch64-linux-android30-clang` to match `min_sdk_version = 30`.
  - The headless toolchain needs only env vars: `ANDROID_HOME`/`ANDROID_SDK_ROOT`
    (`$GITHUB_WORKSPACE/android-sdk`), `ANDROID_NDK_HOME` (=SDK/NDK/25.2.9519653),
    `JAVA_HOME` (JDK 17 via `actions/setup-java`), rustup
    `aarch64-linux-android`. Install via `sdkmanager`:
    `platforms;android-34`, `build-tools;34.0.0`, `platform-tools`,
    `ndk;25.2.9519653`. (The 0.7.10 android template defaults compileSdk/targetSdk
    to 34 with no `[android]` override, hence android-34 not android-33.)
  - **License acceptance on the runner:** `yes | sdkmanager --licenses` reads
    'y' forever; once sdkmanager stops reading, `yes` dies with a benign
    SIGPIPE (141), and under `set -o pipefail` that alone would abort the step
    — but `sdkmanager --licenses` can ALSO genuinely exit 1 on a headless
    runner (its interactive prompt needs a tty). The step therefore never lets
    that line abort: it surfaces sdkmanager's own tail on a real error and, if
    no `${ANDROID_HOME}/licenses/android-sdk-license` was produced, writes the
    well-known accepted hashes directly:
    `android-sdk-license` =
    `8933bad161af4178b1185d1a37fbf41ea5269c55` + `d56f5187479451eabf01fb78af6dfcb131a6481e`
    (SDK) + `24333f8a63b6825ea9c5514f83c2829b004d1fee` (NDK);
    `android-sdk-preview-license` = `84831b9409646a918e30573bab4c9c91346d8abd`.
    The subsequent `sdkmanager "ndk;…"` install still enforces acceptance, so a
    broken fallback fails there loudly instead of silently.
  - Signing (Phase 7 cutover): stable release key from repo secrets
    (`ANDROID_KEYSTORE_B64/PASSWORD/ALIAS`, RSA-2048 PKCS12, 30y, alias
    `spotifydx` — rotated 2026-09-10 after a store/key password mismatch
    killed a release; private key exists ONLY in Secrets) + `apksigner sign`
    from `build-tools/34.0.0` on the unsigned owned-app release APK; verify
    with `apksigner verify`. Conventions that prevent repeats: SINGLE hex
    password for store + key (no shell-quoting hazard, halves mismatch
    surface), and a `keytool -list` guard right after decode so a broken
    secret fails with "B64 corrupt / wrong PASSWORD / wrong ALIAS" instead
    of apksigner's opaque "Wrong password?". The job FAILS LOUDLY when
    secrets are missing — per-release keystores silently break the updater
    (signature mismatch), so no throwaway fallback. Asset keeps the
    updater's basename `app-release-unsigned-signed.apk` (ANDROID_TOKEN).
  - Version stamping: `SPOTIFY_DX_VERSION_CODE=${{ github.run_number }}`
    (monotonic — updates require a rising code) and
    `SPOTIFY_DX_VERSION_NAME=${GITHUB_REF_NAME#v}` into
    `android/app/build.gradle` env-driven `versionCode/versionName`
    (local defaults stay 1/0.1.0). Groovy gotcha: chained
    `(System.getenv(..) ?: '1').toInteger()` mis-evaluates to null on
    AGP 8/Gradle 9 — use explicit `def` + ternary.
  - `scripts/build-kotlin.sh` runs Gradle `--offline` by default (local loop
    installs nothing); CI exports `GRADLE_OFFLINE=0` for first-time dep
    resolution. The script's bridge-symbol compat check is the §13 CI gate.
  - Phase 7 cutover (2026-09-09): the `android-apk` job builds the OWNED
    Kotlin app (`android/`) — no `dx build`, no `stage-updater.sh`, no NDK
    symlink step (the script wraps the toolchain internally, including an
    `aarch64-linux-android-ar` → `llvm-ar` wrapper: cc-rs/ring do NOT read
    `CARGO_TARGET_*_AR`, they probe `ar` on PATH — this exact gap broke CI).
    Legacy mobile renderer removal stays deferred per spec (one clean release
    first).
  - `dx build --platform web --release` writes the site to
    `target/dx/<crate>/release/web/public` — **`out_dir` is NOT honored for `dx
    build`** (DioxusLabs/dioxus#3328), so the `web` job packages from that path,
    not a repo-root `dist/`.

### 6.9e In-app updater + user profile (kal port, Phase C/D)

- **GlobalSignal statics have NO `&mut self` `set()` in dioxus 0.7** — calling
  `STATIC.set(x)` fails E0596 with "cannot borrow immutable static item as
  mutable". Assign through the write guard instead:
  `*UPDATE_STATUS.write() = Some(msg);`, `*PROFILE_ERROR.write() = msg;`,
  `*UPDATE_READY.write() = true;`. (`Signal::set` exists only on a `&mut`
  binding, which a `static` never gives you.)
- **Platform asset matching is substring-based, token = platform-tail only.**
  Published assets are version-embedded (`spotify-dx-v0.1.8-x86_64-unknown-linux-gnu.tar.gz`)
  AND have an unversioned alias (`spotify-dx-x86_64-unknown-linux-gnu.tar.gz`),
  so the token must match both — e.g. `LINUX_TOKEN = "x86_64-unknown-linux-gnu.tar.gz"`
  (no `spotify-dx-` prefix). Android's asset is not version-prefixed:
  `app-release-unsigned-signed.apk`. Verified against the live release via the
  GitHub API before locking these in.
- **`assert!(name.contains(LINUX_TOKEN))` needs the versioned form** in tests;
  the naive `spotify-dx-x86_64-…` name never occurs as the full asset name in
  the wild.
- **Safe JNI without `unsafe` (`#![forbid(unsafe_code)]`):** kal uses
  `ndk_context` + `JavaVM::from_raw` (unsafe) — NOT usable here. On Android we
  get `getFilesDir()`/file paths and fire the install intent through
  `wry::prelude::dispatch`. Verified signature in wry 0.53.5
  (`src/android/mod.rs`): `dispatch<F: FnOnce(&mut JNIEnv, &JObject, &JObject) +
  Send + 'static>`. The JNI strict-grace bit: wrap a `JString` in a `let` before
  `env.get_string(&jstring)` or you hit E0716 (temporary `JString` dropped while
  borrowed).
- **`build.rs` version injection:** Cargo.toml stays at `0.1.0` while releases
  are tagged `v0.1.x`; `SPOTIFY_DX_VERSION` is wired
  `SPOTIFY_DX_RELEASE_VERSION` env (set in CI from `github.ref_name`) →
  `git describe --tags --abbrev=0` → Cargo version, leading `v` stripped.
  `updater::CURRENT_VERSION` is `env!("SPOTIFY_DX_VERSION")`; CI adds a build
  step. Never bump Cargo.toml's version to match a tag.
- **Android self-update plumbing** (the `android-apk` CI job): after
  `dx build --platform android`, run `scripts/stage-updater.sh`, which copies
  `android/updater/src/main/kotlin/…/SpotifyDxUpdater.kt` +
  `SpotifyDxFileProvider.kt` into the `app/src/main/kotlin` tree (idempotent
  `cp -n`) and patches the debug AGP manifest (python3, marker
  `<!-- spotify-dx-updater:provider -->`) with the FileProvider
  (`authorities="com.spotifydx.app.updates"`, `file_paths`=filesDir/updates).
  The Rust side calls `com/spotifydx/app/SpotifyDxUpdater.installApk` via JNI →
  `Intent.setDataAndType(FileProvider.getUriForFile(…, "updates/spotify-dx-update.apk"), "application/vnd.android.package-archive")`.
  Keep the Kotlin class name + method name in sync with `fire_install_intent`.
- **The updater is desktop + Android only — wasm stubs it out.** Desktop deps
  `tar` + `flate2` live under
  `[target.'cfg(all(not(target_os = "android"), not(target_arch = "wasm32")))'.dependencies]`
  and the binary staging path is cfg'd the same way (Android downloads the APK
  instead of a tarball). Windows' asset is a zip the tar-based stager can't
  open — `stage_desktop_binary` returns an error message rather than panicking.
- **Profile is pure-local client state** (no server): JSON in
  `{data_dir}/profile.json` next to `settings.json`. Avatar is base64 in JSON;
  validated by PNG/JPEG/GIF/WebP magic bytes, 4 MiB cap. Web keeps parity via
  `platform::storage` (localStorage). It's also shown in the top bar (avatar or
  first-initial dot + display name).
- **Settings now persists `auto_check_updates` (default on).** The App does a
  one-time startup check gated on a `update_checked` signal so a rerender
  doesn't re-fire; "Apply update now" is rendered only when `UPDATE_READY`.
- **`profile::init()` must run INSIDE the dioxus runtime.** `bootstrap()` in
  `main.rs` executes before the runtime exists — writing any `GlobalSignal`
  there panics: "Must be called from inside a Dioxus runtime"
  (dioxus-core `runtime.rs:100`). `apply_staged_update()` stays in `main()`
  (pure file IO, no signals); `profile::init()` runs behind a one-shot gate in
  the `App` component's effects, next to the player/theme/update one-time boots.
  Keep any future pre-runtime init signal-free.
=== 6.9f wasm: tracing's default timer panics (white screen) ===

- **`wasm32-unknown-unknown` has NO `std::time::SystemTime`.** Any call to it
  panics with `time not implemented on this platform` and, because wasm builds
  use `panic = "abort"`, the panic becomes a bare `RuntimeError: unreachable` /
  `wasm-bindgen: imported JS function that was not marked as catch threw an
  error` — a completely blank `#main` with no Rust message in the console. The
  whole app aborts before first render.
- **Root cause here: `tracing_subscriber::fmt()`'s default timer uses
  `SystemTime::now()`**, so the very FIRST `tracing::info!`/`warn!` after `.init()`
  (e.g. `adblock: engine thread spawned` in bootstrap) panics and aborts the app.
  Fix: on wasm build the logger with `.without_time()` (see `main.rs::init_logging`).
  Do NOT re-add a timestamp to wasm tracing.
- This also breaks `chrono::Utc::now()`/`Local::now()`, `std::time::Instant`
  (in `std::time` on wasm-none these are similarly unavailable/abort) and every
  `SystemTime::now()` call site (`spotify/store.rs`, `media/images.rs`,
  `streaming/cache.rs`, the page-art `SystemTime` seeds). Audit wasm-reachable
  paths before adding time APIs; use `js_sys::Date::now()` (`Date.now()`) for
  monotonic/epoch time on web.
- **Reproducing/scoping a wasm panic locally:** serve the bundled `public/`
  tree so the baked `base_path` resolves (e.g. `_deploy` under a
  `spotify-dx/app/` dir via `python3 -m http.server`), drive it with
  headless Chromium (`--remote-debugging-port`) + a CDP websocket
  (`Runtime.enable`, watch `Runtime.exceptionThrown` +
  `Runtime.consoleAPICalled`). `panic=abort` strips the message; temporarily
  `std::panic::set_hook` (web-sys `console::error_1` + inject a `<pre>` into
  `document.body`) to surface the message, then remove it. The
  `RuntimeError` stack's wasm-function indices are per-build and useless for
  cross-referencing, so prefer the panic message over the stack.
- Verify the whole matrix after touching updater/profile: `cargo check
  --features desktop` + clippy, the Android `cargo clippy --no-default-features
  --features mobile --target aarch64-linux-android` (NDK bin on PATH), the host
  `mobile` host build, the wasm web build, and `cargo test`.

### 6.9g Web login is CORS-blocked on static hosting — future backend proxy plan

**Confirmed 2026-09** on the deployed site `https://mihneamoso.github.io/spotify-dx/app/`:
the web build loads, the wasm `time` panic is fixed (§6.9f), and the login gate
now shows `Couldn't start Spotify login: could not read the Spotify session
(CORS or logged out?): fetch error: JsValue(TypeError: Failed to fetch ...)`, plus
a console flood of
`Access to fetch at 'https://open.spotify.com/get_access_token?reason=transport&productType=web_player'
from origin 'https://mihneamoso.github.io' has been blocked by CORS policy` and
`GET ... net::ERR_FAILED 429 (Too Many Requests)`.

**Why the browser path cannot work (platform facts, not our bug):**
1. `sp_dc` / `sp_key` are **HttpOnly**, so no page JS (not even the
   `open.spotify.com` login tab's own scripts) can read them via
   `document.cookie` — they cannot be exfiltrated and "sent back" to the app tab.
2. Spotify's `/get_access_token` and `/api/token` endpoints send **no
   `Access-Control-Allow-Origin`**, so a credentialed cross-origin `fetch` from
   `github.io` is hard-rejected by the browser regardless of cookies. The
   desktop/mobile WebView avoids this because its page IS `open.spotify.com`
   (same-origin). The 429 is the rate-limit on the app's repeated `capture_session`
   attempts contributed by the login gate's retry-loop.
3. Two browser tabs on **different origins** (`github.io` vs `open.spotify.com`)
   share neither `localStorage`, `BroadcastChannel`, nor cookies, and cannot run
   a callback across the boundary.
4. We cannot inject the desktop `POLL_JS` (§6.9b) into the `open.spotify.com` tab
   from the app tab.

**So the only way to give the browser a web-player session is a server-side
proxy** that holds the user's session cookie and mints tokens on their behalf
(server-to-server calls bypass browser CORS entirely).

#### Target backend-proxy architecture (future implementation)
- **Hosting:** a small serverless / edge function (Vercel, Cloudflare Workers,
  or a tiny always-on host), NOT GitHub Pages (static only). Must support
  POST + configurable CORS headers + a small secret.
- **Session entry:** the browser cannot read HttpOnly `sp_dc`, so the first-login
  UX still needs the user's consent flow. Two options to plan:
  - (preferred) **Browser same-origin relay:** the proxy exposes an endpoint the
    user visits once in the tab that pages to `https://open.spotify.com`. The
    proxy server then reads the browser-sent `sp_dc` cookie on the
    `open.spotify.com` request (cookies DO travel with the request; the server,
    not JS, reads them). Server stores it server-side keyed by a session id it
    returns to the browser. This avoids any client-side cookie reading.
  - (fallback) user pastes their `sp_dc`/`sp_key` into the app once; stored
    server-side + encrypted.
- **Token minting:** the server calls Spotify's `/api/token` (TOTP) or
  `/get_access_token` with the stored cookie, then returns the token + expiry
  with `Access-Control-Allow-Origin: <app origin>` and a short TTL. The wasm
  app fetches the app-origin proxy endpoint instead of `open.spotify.com`.
- **Refresh:** reuse the same `refresh_token` path through the proxy on expiry.
- **Security:** never expose the cookie to client JS; use a server-held keystore,
  per-user rotating secret, HTTPS-only, and short-lived tokens. Caveat: doing
  live playback server-side is its own project — this proxy only mints the
  session/access tokens the existing player stack already consumes.
- **Migration steps (when approved):** add a `web_backend.rs` platform module
  (wasm-only) that talks to the proxy; land the serverless function + deploy
  config in a `web-backend/` dir; gate on a `SPOTIFY_DX_WEB_BACKEND_URL` build
  var; update `auth/mod.rs` non-native `login()` + `web_login.rs` to use it;
  keep a graceful local fallback for dev.

### 6.9h Kotlin migration — Phase 0 + Phase 1 (native core library + owned Kotlin app)

Status: **Phase 0 gate green, Phase 1 shell smoke builds** (`docs/KOTLIN_MIGRATION.md`
§14). The Kotlin app in `android/` is the primary Android build path
(`scripts/build-kotlin.sh` → `scripts/build-android-install.sh` default;
`DX_LEGACY=1` keeps the old dx renderer until the Phase 7 cutover). The
dioxus-mobile Rust code is untouched and still builds.

- **Core extraction shape (deliberate deviation from §5):** `app` + `ui` live
  in the LIB (`src/lib.rs`), not binary-only. Reason: `app.rs`/`ui/*` use
  `crate::` paths that only resolve inside one crate, and `ui/theme.rs` +
  page tests must keep running on every feature combo including headless.
  The `.so` carries dormant UI code until Phase 7 removes the legacy
  renderer — accepted, documented in `KOTLIN_MIGRATION.md` terms as
  "binary is a thin shell over the library" (true: `main.rs` is entry points
  + `bootstrap()` only).
- **The headless core builds for Android WITHOUT the `native` feature:**
  `cargo build --no-default-features --target aarch64-linux-android` is the
  Kotlin lib config. Three re-gates made it compile: `platform::android_views`
  / `android_webview` are now `all(target_os="android", feature="native")`
  (dioxus-renderer view layering — the Kotlin app owns its own in
  `MainActivity`/`LoginWebView`); `updater::android_files_dir` /
  `request_android_install` split into native (wry `dispatch`) vs headless
  (`set_bridge_files_dir` + `request_android_install_with(env, activity)`)
  variants; `updater::apply_now`'s android branch is native-only.
- **`#![forbid(unsafe_code)]` vs JNI exports:** current rustc classifies
  `#[no_mangle]` under `unsafe_code`, so a crate-level `forbid` rejects every
  `Java_…` export with no per-item override. `src/lib.rs` is therefore
  `#![deny(unsafe_code)]` (still a hard error for `unsafe {}` anywhere) with
  `#[allow(unsafe_code)]` scoped to the bridge's `#[no_mangle]` attributes
  only — verified: the allow silences the export lint while an `unsafe {}`
  block elsewhere still fails. `main.rs` (bin) keeps `forbid`. The bridge
  contains zero `unsafe` blocks — only the safe `jni` 0.21 API.
- **jni 0.21 plumbing facts:** `JNIEnv<'local>` methods are mixed-receiver:
  `new_string`/`convert_byte_array` take `&self`, but `get_string`/
  `find_class`/`call_static_method` need `&mut self`. `JString` is a
  raw-pointer wrapper (frame lifetime, not a borrow of `env`), so bridge
  helpers take `env` by value: `guarded<'a>(mut env: JNIEnv<'a>,
  f: impl FnOnce(&mut JNIEnv<'a>) -> String) -> JString<'a>`. Every entry is
  panic-guarded (`catch_unwind` → `BRIDGE_PANIC` envelope, never unwinds
  into the JVM). Kotlin calls blocking methods from `Dispatchers.IO` only.
- **Signal-entangled services can't run headless:** dioxus `GlobalSignal`s
  panic outside a runtime (`Runtime::new` is `pub(crate)` — no way to host
  one from the bridge), and `session::ensure_token` gracefully errors there
  by design. So the bridge implements only signal-free surfaces for real
  (settings/profile file CRUD via new `profile::persist_to` + `util::`
  dir overrides for `filesDir`/`cacheDir`, adblock engine, updater
  network+staging, token-store, session mirror + event queue) and exposes
  the exact §6.1 session/data/playback signatures as `PHASE_2_PLUS` stubs
  (macro-generated) so Kotlin compiles against the final contract now.
- **Owned Gradle project (`android/`, no generator, no Studio):** Groovy DSL
  for the app module ON PURPOSE — the offline cache holds AGP 8.7.0 + KGP
  2.0.20 jars but not the `plugins {}` marker artifacts, so the classic
  `apply plugin` style is what resolves with `--offline`. Wrapper pins
  Gradle 9.1.0 (cached dist; AGP 8.7.0 + KGP 2.0.20 configure fine on it).
  Deps are pinned to cached versions only (appcompat 1.7.1, material 1.13.0,
  activity 1.8.0, fragment 1.5.4, lifecycle 2.6.2, recyclerview 1.2.1,
  webkit 1.13.0, coroutines 1.6.4) — notably NO `-ktx` artifacts (not
  cached): fragments use `ViewModelProvider` directly and ViewModels extend
  `ScopedViewModel` (same viewModelScope semantics). No Compose/Media3
  (not cached): classic Views + Material3 + framework WebView/MediaPlayer/
  MediaSession. First build on a fresh cache needs ONE online run (Gradle
  metadata versioning); after that `--offline` holds.
- **`scripts/build-kotlin.sh` runs the bridge-compat check (§13):** every
  `Java_com_spotifydx_app_CoreBridge_*` symbol referenced from Kotlin must
  exist in `libspotify_dx.so` (`nm -D`), else the build fails here instead
  of at runtime. The staged `.so` under
  `android/app/src/main/jniLibs/<abi>/` is git-ignored (rebuilt every time).
- **`android/updater/` staging sources are now first-class:** copied verbatim
  into the owned project; `scripts/stage-updater.sh` is legacy-path-only.
- **Login capture JS (`CaptureJs.kt`) is `POLL_JS` verbatim** (TOTP key,
  `0x98BADCFE`, fetch hook, relay, message vocabulary) with `window.ipc`
  backed by the `SpotifyDx` JavascriptInterface instead of the title ferry.
  Do not "simplify" either copy — and keep both in sync if the endpoint
  contract changes.
- **Material `setSelectedItemId` dispatches the selection listener
  UNCONDITIONALLY** (even when the id is unchanged). A shell that writes the
  selected id on every navigation (`syncNav`) therefore recurses
  select → go → syncNav until `StackOverflowError` (verified on-device: 168
  repeating `go`/`syncNav` frames, overflow surfacing in `findViewById`).
  Guard both sides: only assign when different, and ignore selections already
  current. Caught by launching on a real device — the emulator-less sandbox
  never exercises this.
- **Never ship `./gradlew assembleDebug` alone: always install via
  `scripts/build-kotlin.sh`.** Gradle never rebuilds Rust, so a lone assemble
  silently bundles the previous `jniLibs/libspotify_dx.so` — the symptom is a
  fully-built APK whose new bridge calls return `PHASE_2_PLUS` (a `Phase 2+`
  toast + no login page, in the case that caught this). The script's
  bridge-compat check only verifies symbols exist, not that the `.so` is
  fresh — cargo's incrementality is what keeps it fresh; skipping cargo
  defeats it.
- **Airplane mode kills wireless adb.** The device drops off `adb devices`
  the moment offline mode goes on (and the PIC between shell and phone dies
  with it) — plan offline tests as fire-and-observe-from-the-phone, and
  re-connect after. Offline cold start shows Spotify's own reCAPTCHA/notice
  UI (genuine page ⇒ same errors as today by construction); hard main-frame
  failures additionally toast + keep the gate retry.
- **Phase 2 refresh inversion (documented, not a hack):** the session page
  lives in Kotlin, so refresh initiates there — `bridge::refreshToken` only
  judges the mirror (fresh / `NEEDS_PAGE`), and `SessionRefresher` (single-
  flight shared `Deferred` = the native fan-out semantics) revives the page,
  captures, and retries. The raw token never crosses JNI. `currentUser` is
  the silent-restore verifier over the mirrored token (`/v1/me`, 401 →
  `SESSION_EXPIRED` transition). Expiry recovery is watchdog-timer-driven
  (30s cadence, 5min horizon), never token-reactive.
- **Horizontal shelves (desktop `.shelf-row` parity):** Home playlists use
  `TitleAdapter` with `itemLayout = R.layout.item_card` (150dp card, square
  art on top, same `title_art/title_main/title_sub` IDs so binding is shared)
  + `LinearLayoutManager(HORIZONTAL)`. Search/Library keep the default
  `item_title` vertical rows. Gotcha: adding the 2nd constructor param broke
  all trailing-lambda `TitleAdapter { pos -> ... }` call sites (lambda binds
  to the last param) — they must use named `onClick = { ... }`.
- **Phase 5 SDK path blocked by platform (verified on-device 2026-09):**
  Android WebView has no EME keysystem, so the Web Playback SDK fails init
  (`EMEError: No supported keysystem`, `init_error`) and never reaches
  `ready` — regardless of account tier. The driver (`SdkWebViewDriver`)
  fails fast (`bootFailed` short-circuits `awaitDevice`; errors route via
  `onError`, toasted only when an SDK session is active) into the open
  engine, which then plays normally. Do not retry SDK boot or add fallback
  timers around it; the failure is permanent. A native Android SDK (App
  Remote) would be a separate integration, not this path.
- **Fragment commits from async collectors must allow state loss:**
  `MainActivity.go()` uses `commitAllowingStateLoss()` — session/watchdog
  flows can emit after `onSaveInstanceState` (backgrounded app) and plain
  `commit()` crashes there (`IllegalStateException`, seen 2026-09-09).
- **Caching layers (Phase 8 batch, 2026-09-09):**
  - Artwork = Coil 2.6 (`ArtworkLoader.load` API unchanged): 25% memory +
    100MB disk (`cacheDir/artwork`), custom `CoreGateFetcher` reads bytes
    through the core gate (base64 → bytes) so ad-filter + core disk cache
    stay intact — never direct HTTP. First build after adding a dep must run
    ONLINE once to seed `~/.gradle` (local loop stays `--offline`).
  - Music data = `android.util.LruCache(32)` + 5min TTL in `MusicRepository`
    (successes only, JSON-string snapshots); cleared in
    `SessionRepository.logout`. Core disk SWR stays the cross-restart layer.
  - Persisted state = Room `spotifydx.db` v1 (`AppDb.kt`): `search_history`
    (LRU 20), `queue_items` (ordered snapshots), `playback_state`
    (track + position + wall-clock). `SearchHistory`/`PlaybackStore` +
    `AppState` ctx holder; `PlayerRepository.restore()` rehydrates paused
    (never autoplay); queue writes debounced, last-played on track/pause/seek
    (never the 250ms ticker). Track snapshots via `Models.trackToJson`
    (core-shaped, `Models.track` parses back).
  - Long-press enqueues on all five track lists (was wired to nothing).
- **Cold-start theme:** `setTheme` paints from the in-memory default before
  the async store load; `MainActivity` recreates exactly once when the loaded
  theme disagrees (`appliedTheme`/`themeReconciled` — no loop: the recreated
  instance paints the loaded value).
- **Gestures (2026-09-10):**
  - Swipe-to-queue (`SwipeToQueue.kt`, Spotify parity): `ItemTouchHelper`
    LEFT|RIGHT on all five track lists, 0.4 commit threshold, green
    (`spotify_green`, mirrors the CSS token) rounded reveal + queue-add icon
    with threshold-ramped alpha; `onSwiped` enqueues + `notifyItemChanged`
    springs the row back (dataset unchanged). Replaced long-press (removed
    from `TrackAdapter`); shared `queueWithToast` action. XML comments must
    not contain `--` (resource merger rejects it).
  - System back mirrors the top-left button: sub-screen → HOME via
    `onBackPressedDispatcher.addCallback` (no fragment back-stack exists —
    `go()` replaces without adding); HOME/GATE keeps platform exit (else
    back could never leave the app). Two gotchas, both "back exits from a
    sub-screen": (1) never `isEnabled = false` + redispatch in the callback
    — a disabled callback stays dead for the activity instance; use
    `finish()` for the exit branch. (2)     `current` must survive recreation
    (`onSaveInstanceState`, resync in `onCreate`) — rotation/theme-recreate
    restores the visible fragment but resets the field to GATE.
- **Playlist/album >100 tracks (fixed 2026-09-10, user-verified on-device):**
  `gql_playlist`/`gql_album` sent a single page (`offset: 0, limit: 100`).
  Now page 0 reveals the total, remaining offsets fan out via
  `try_join_all`, assembled into the unchanged `TracksMeta`/album shape —
  zero bridge/Kotlin/desktop changes. Rules: `PAGE_LIMIT`/`MAX_PAGES`
  (10k-track ceiling), page failure fails the whole call (never gappy
  lists), offsets advance by requested amounts (parse-skips don't shift
  windows), `total_tracks` stays the rendered count. Pure helpers
  (`remaining_offsets`, `playlist_total`, `album_total`, page parsers) are
  unit-tested with canned payloads. Pre-existing failure note:
  `ui::theme::app_shell_grid_wires_every_shell_zone` fails on the pristine
  tree too — unrelated.
- **Playlist sort (2026-09-10, all 5 orders verified on-device):**
  `DetailViewModel.SortOrder` (CUSTOM/TITLE/ARTIST/ALBUM/RECENT) over a
  pristine `original` list (no drift); stable in-memory sorts, button +
  checkable `PopupMenu` next to Play, playlist-only (albums/artists keep
  natural order). `Track.added_at` (ISO string, lexicographic ==
  chronological) threaded from the item element — GQL shape gotcha: it is
  an OBJECT `{isoString}` on the element (sibling of `itemV2`), NOT a
  string and NOT inside the track node (two temp-diag builds proved it;
  verify unknown GQL shapes via key/value logcat before guessing).
  `RECENT` sinks untimestamped rows; Kotlin `Track.addedAt` round-trips
  through queue snapshots.
- **Provider expansion Phase A (YouTube hardening, 2026-09-10):**
  `youtube.rs` tries ranked query variants (`artist title audio` →
  `title artist` → `title artist topic`; later variants run only when
  earlier yield nothing — happy path stays one search), collects up to 6
  deduped candidates with `lengthText` durations, duration-gates (±15s;
  unknown durations accept either side — no regression), and walks
  candidates: transport errors abort (no timeout multiplication), content
  errors advance. Ciphered URLs recover via Piped `/streams` for the same
  videoId (primary + 1 fallback; Phase B builds the health-checked pool).
  Pure helpers unit-tested (`search_queries`, `parse_length_secs`,
  `duration_accepts`, `pick_piped_audio`, mappings). Explicit content is
  NOT filtered anywhere (provider sets contentCheckOk/racyCheckOk) —
  misses are catalog gaps, not blocks.
- **Provider expansion Phase B (Piped provider, 2026-09-10):**
  `providers/piped.rs` sits after `youtube`: pinned API hosts + in-process
  health (2 consecutive transport failures → 5min cooldown; content gaps
  never blame the host), 8s client timeout, `/search?filter=videos` →
  duration-gated pick → `/streams/{id}` → best non-video audio (shared
  `pick_piped_audio`/`duration_accepts`/`quality_for_bitrate`/
  `format_for_mime` now `pub(crate)` in `youtube.rs`). Region variance is
  the point: content misses retry the next instance, transport failures
  cool it. Unit-tested (URL parse, urlencode, response shape).
- **Provider expansion Phase D (Audius + SoundCloud, 2026-09-10):**
  `audius.rs` (after `saavn`): keyless `api.audius.co/v1` search →
  duration-gated pick (skips deleted/unlisted) → `/tracks/{id}/stream`
  with redirects DISABLED (302 Location is the signed URL, ~days validity
  >> 50min cache; 200 means proxied bytes). `soundcloud.rs` (last):
  client_id auto-scraped from public JS bundles (24h cache, refresh on
  401/403 + single retry), api-v2 search → skips snippet-only
  (`policy != ALLOW` — a 30s preview cutting off is worse than chaining
  on) → progressive transcoding only (no HLS on the platform player) →
  resolve to direct URL. Pinned live before coding (client_id verified
  working, transcoding + preview behavior confirmed). Unit-tested
  (pickers, regex, shapes).
- **Provider expansion Phases E+F (ISRC enrichment + credential tier):**
  `streaming/isrc.rs`: MusicBrainz recording lookup (strict title, lenient
  artist/length), shared 1 req/s gate + 512-entry cache, miss-path ONLY and
  only when Qobuz is configured. Resolver hook retries the Qobuz consumer
  alone (others can't use ISRC — no full second pass). Cache probe order is
  a shared `CACHE_PROBE_ORDER` const with a unit test (a hardcoded subset
  left new providers' entries write-only — caught here).
  Credentials: 4 opaque fields on core `Settings` (serde-default, `Copy`
  removed — desktop `*peek()` sites now `.clone()`), write-through into the
  `SETTINGS` signal in bridge get/setSettings (Android bypasses the signal
  otherwise), Kotlin store round-trips them (a save omitting keys would
  WIPE them) + password-field UI section. Qobuz revived behind both keys
  (ISRC/text search + `getFileUrl` format 6, documented surface,
  LIVE-VERIFY PENDING with real keys). Tidal stays parked (direct revival
  needs keys to verify); Deezer field reserved, gw_light flow deliberately
  unshipped. Tokens never logged.
- **Lyrics (Phase 1, 2026-09-11):** `streaming/lyrics.rs` over LRCLIB —
  exact `/api/get` (artist/title/album/duration) → 404 → `/api/search` +
  duration match (±10s, synced preferred) → miss is `found:false`, never an
  error. Cached in the core store (immutable data, SWR ideal) with
  single-flight; bridge `fetchLyrics`, `MusicRepository.lyrics()` returns
  null on miss/blank input. Store `resolve()` takes `self` by value —
  call `Store::global().clone()`; loader closures must own (`'static`).
  Wasm bypasses the store (Send-bound futures don't compile there —
  `cargo check --features web` gates it). No UI yet (Phase 5).
- **Player sheet state (Phase 2, 2026-09-11):** `PlayerRepository.State`
  gains `source` (play origin, set at all 8 `play()` sites, preserved on
  resume) + `audioTier` (display label from resolve `{format, provider,
  quality}` — bridge now includes quality). Tier mapper is honest-only
  (`format==flac→FLAC · Lossless`, saavn high→320 kbps, else codec name,
  unknown→hidden; SDK path suppresses). Room v2 `Migration(1→2)` adds
  `source` to `playback_state` (no destructive fallback anywhere —
  version bumps REQUIRE a Migration). Trailing-lambda rule resurfaces:
  added params go BEFORE the lambda param (`fetch` stays last).
- **Player sheet shell (Phase 3, rewritten 2026-09-11):**
  `PlayerSheetController` persistent overlay in `activity_main.xml`
  (`player_sheet` container + `view_player_sheet` include) — Echo Music
  architecture, deliberately NOT a `BottomSheetDialogFragment` (dialog
  windows brought theme/token issues and rendered an empty container
  with zero hierarchy). Slide up/down animations, chevron/back/
  swipe-down minimize, Queue|Lyrics toggle tabs. Reference clone kept at
  `/tmp/opencode/echo-music` (depth-1, for future design overhauls);
  Echo Music is GPL-3.0, credited in README (patterns only, no code —
  Compose vs Views). UNVERIFIED on device (phone in use during session).
- **System media artwork (2026-09-12, verified via dumpsys):**
  `PlaybackService` attaches the track bitmap to the notification
  (`setLargeIcon`) + session (`METADATA_KEY_ALBUM_ART`). Two rules: (1)
  key the art cache on `coverUrl`, NOT track id (ids can be empty per
  source — the cache collapsed and art never loaded); (2) ONE bitmap key
  at ~320px — the same bitmap under 3 keys triple-parcels toward the 1MB
  binder limit. `dumpsys media_session` "size" counts bundle entries:
  base 4 (title/artist/album/duration) + 1 per bitmap key (size=5 means
  art attached, NOT missing). Art comes from the same core gate as the UI
  (`MusicRepository.artwork` base64 → downsampled decode), one in-flight
  fetch with stale-track guard, re-publishing both surfaces on landing.
- **Queue reorder (Phase 4, 2026-09-11, compile-verified):**
  `QueueDrag` (`ItemTouchHelper` UP/DOWN, separate helper coexisting with
  swipe's fling gestures) on both queue lists (Queue screen + sheet);
  long-press lifts (0.7 alpha), drop commits via `moveQueue()` (bounds +
  no-op guarded) into state AND the existing debounced persist — order
  survives restarts. Manual `notifyItemMoved` gives live feedback while
  the StateFlow `submitList` reconciles after.
  (SUPERSEDED 2026-09-12 by the unified timeline below — kept for history.)
- **Unified queue timeline + session drag (Echo-modeled, 2026-09-12,
  verified on-device with adb draganddrop + dumps + restart):**
  one list (PAST played + NOW current + NEXT upcoming, `RowKind`/
  `QueueEntry` in Models.kt, `PlayerRepository.timeline()`), shown in the
  Queue screen AND the sheet's expanded queue. Drag rules, all learned the
  hard way: (1) move DATA synchronously with the views during the drag
  (`TrackAdapter` session-local entries + `moveVisual`, Echo's
  `mutableQueueWindows` — mirroring bare `notifyItemMoved` against static
  data goes stale mid-drag and lands scrambled/inverse orders); (2) commit
  ONCE on drop (`commitTimeline` splits by kind, single emit+persist);
  (3) mid-drag state emits (250ms ticker!) stash WITH kinds
  (`pendingEntries`) and ALWAYS drain on drop — even zero-move drops —
  or the stash poisons the next drag's session base (the snap-back +
  history-drains-into-queue bug); (4) NOW is a pinned anchor (no handle,
  cannot displace, tap toggles) so cross-current moves can't disturb
  playback — Echo's rapid song-switching on such moves is structurally
  impossible (`track` lives outside the reorderable list). Drags start
  instantly from an explicit handle (`ic_drag_handle`, handle touch →
  `startDrag`, long-press drag OFF). Handles show only on armed lists.
  History (Room v3 `history_items` + `MIGRATION_2_3`, cap 50, debounced
  persist): pushed on advance/play, jump-back truncates, survives
  restarts. Test harness notes: `adb shell input draganddrop` drives real
  ItemTouchHelper drags; uiautomator dumps FAIL during playback (ticker
  invalidates idle — pause first) and with infinite marquee (never use
  marquee on always-resident views).
- **Lyrics view (Phase 5, 2026-09-11, compile-verified):** `Lrc.kt` (pure
  `[mm:ss.xx]` parse + binary-search `indexAt`), in-sheet container with
  three mutually exclusive views (synced list / plain scroll / state
  message). Fetch-per-track via `MusicRepository.lyrics()` with
  fragment-scoped cache + stale-track guard; highlight updates surgically
  (prev+new rows only) with auto-scroll on line change; states SYNCED /
  PLAIN / INSTRUMENTAL / UNAVAILABLE / LOADING / IDLE.
- **wasm breakage pattern (fixed 2026-09-10):** `reqwest::redirect` does not
  exist on wasm (browser owns redirects) — `bare_client` and redirect logic
  are `cfg(not(wasm))`; the wasm path uses `resp.url()` after following.
  New provider code MUST be checked with
  `cargo check --no-default-features --features web --target
  wasm32-unknown-unknown` (toolchain installed locally) — desktop/android
  checks do not catch wasm-gated API absences. Same for warning hygiene
  (e.g. cooldown consts unused on wasm need cfg-gating).
- **Dioxus signals are FORBIDDEN on bridge threads (fixed 2026-09-11):**
  even `write()` panics ("Must be called from inside a Dioxus runtime")
  outside the runtime — surfacing as BRIDGE_PANIC on every call. This broke
  ALL playback (qobuz `is_available` read the signal per resolve) AND all
  settings load/save (bridge write-through), which also explains the
  theme-switch churn. Rule: bridge/provider code uses ONLY the runtime-free
  mirrors in `settings.rs` (`sync/stream_credentials`,
  `set/session_premium`), synced from bridge get/setSettings +
  currentUser/notifySession/logout. `peek()` is equally suspect — prefer
  the mirrors. `guarded` now captures the panic message + backtrace into
  logcat (envelope carries the first 300 chars).
- **Provider expansion Phase C (JioSaavn direct, 2026-09-10):**
  `providers/saavn.rs` after `piped`: FIRST-PARTY `api.php`
  (`search.getResults`, songs-only bucket) — never wrapper deployments.
  DES-ECB decrypt of `encrypted_media_url` with the API's public static key
  (`38346591`; `des`+`cipher` crates) → `_96`→`_320` quality swap (160 when
  not advertised 320). Skips withdrawn items (`disabled`, non-zero
  `rights.code`) and duration-gates; own 8s client + outage cooldown.
  Pinned live: endpoint shape, key, and template verified against real
  responses before coding (wrong remembered keys do exist — verify, don't
  trust memory). Unit-tested incl. a fixed live decrypt vector.
- **Release pipeline cut over (Phase 7, pending first tagged release):**
  `release.yml` `android-apk` now builds the owned Kotlin app with a stable
  key (see §6.9d). Until a tag is pushed and the published
  `app-release-unsigned-signed.apk` is confirmed installable-as-update, the
  updater's apply step remains live-untested. Verified 2026-09: check path
  live ("Up to date (v0.1.10)"), settings survive force-stop, local
  `assembleRelease` + version stamping green (99999/9.9.9-test in aapt).

## 7. Testing

- Unit tests are network-free and live next to the code (`#[cfg(test)]` in
  `spotify/mod.rs`, `adblock/mod.rs`, etc.).
- Run `cargo test` and `cargo test --no-default-features`. Do NOT run the app.

## 8. Updating this file

Whenever a change touches conventions, dependencies, architecture, or reveals
a gotcha, update the matching section here in the same change. The aim: the
next agent reads `AGENTS.md` + `RULES.md` and avoids the mistakes documented
above (especially around `dx serve`, the wry/dioxus versions, and CSS-in-the-binary).