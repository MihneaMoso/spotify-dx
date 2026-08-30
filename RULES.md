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
| `main.rs` | Per-renderer entry points (`desktop`/`web`/`mobile`/headless) + shared `bootstrap()`, logging init. Desktop sets up the window via `dioxus::desktop::Config` + `WindowBuilder` and launches with `LaunchBuilder::desktop()`. |
| `app.rs` | Root `App` component: injects the stylesheet, login gate vs. routed shell, seeds auth from boot snapshot, calls `player::init()` + `player::on_authenticated()` once. |
| `state.rs` | Global signals (single source of truth): `APP_STATE`, `PLAYER_STATE`, `AUTH_STATE`, `ADBLOCK_STATS`, `APP_ERROR`. Plus `Page`, `RepeatMode`, and the state structs. |
| `app_error.rs` | `AppError` enum (thiserror). |
| `auth/` | Web-session sign-in: `webview_login.rs` (desktop GTK window hosting `open.spotify.com`, cookie capture via the internal `get_access_token` endpoint), keychain persistence (`token_store.rs`), refresh/init flows (`mod.rs`). |
| `spotify/` | API models (`models.rs` — incl. `SavedTrack` envelope for `/me/tracks`), filtered HTTP client (`client.rs`), endpoints (`api.rs` — thin wrappers delegating to GQL), GraphQL persisted-query client (`gql.rs` — `api-partner.spotify.com/pathfinder`, used for user playlists + liked songs + saved albums + home + search + album/artist detail), playback endpoint (`player_api.rs`), session helpers (`session.rs`), request store (`store.rs` — in-flight coalescing, memory TTL, disk SWR). |
| `adblock/` | Brave-style ad-block engine (`engine.rs` — `adblock` crate `Engine` on a dedicated `!Send` thread with `mpsc` channel IPC), blocklist fetch/cache (`adguard_api.rs`), cosmetic CSS scaffold (`mod.rs::cosmetic`). Facade: `should_block(url)`, `record_drop()`, `stats_snapshot()`. |
| `player/` | `mod.rs` dispatch (native renderers → webview_bridge; wasm → Connect API), `playback_sdk.rs` (embedded SDK HTML/JS), `webview_bridge.rs` (hidden WebView + IPC), `engine.rs` (`PlaybackEngine` trait). `should_use_open_engine()` checks `EnginePreference`; `play_uri()` routes to `open_play_uri()` via the streaming engine. |
| `ui/` | `router.rs` (incl. `/liked`, `/queue`, `/settings`), `theme.rs` (tokens mirrored from CSS + drift-guard tests incl. the custom-property linter), `icons.rs` (inline SVG), `components/`, `pages/`. |
| `ui/components/` | `app_layout.rs` (shell + sidebar resize), `top_bar.rs` (history/search/avatar menu), `nav.rs` (`SideNav`/`BottomNav`), `now_playing.rs` (right column), `player_bar.rs`, `progress_bar.rs`, `primitives.rs` (`SectionHeader`/`HeroHeader`/`TrackTable`/`SkeletonShelves`), `album_art.rs`, `card.rs` (MediaCard w/ `extra_class`), `track_row.rs`, `toast.rs`. |
| `ui/pages/` | `login.rs`, `home.rs`, `search.rs`, `library.rs`, `liked.rs`, `queue.rs`, `settings.rs`, `playlist.rs`, `album.rs`, `artist.rs`. |
| `media/` | `audio.rs`: symphonia decode (FLAC/M4A/MP3/OGG, seek, gapless planner) with tests. `images.rs`: disk-cached artwork loader (SHA-256 keyed, 128-file LRU, 30-day TTL). `sink.rs`: rodio audio sink thread (`MixerDeviceSink` + `Player` via `rodio::play()`), `SinkCommand` channel, `SinkState` atomics. |
| `streaming/` | Open streaming engine (Phase 4b). `provider.rs`: `Provider` trait, `Resolution` enum, `TrackQuery`. `odesli.rs`: song.link ID mapping (DEAD — public API sunset/401). `cache.rs`: stream-URL cache (memory + disk, 50-min TTL, FIFO 256). `resolver.rs`: cache → provider failover. `providers/{tidal,qobuz,youtube}.rs`: TIDAL & Qobuz DISABLED (`is_available()==false`, Odesli sunset); YouTube (InnerTube ANDROID API) is the sole active, self-contained provider. |
| `settings.rs` | Persistent user settings (`{data_dir}/settings.json`): theme, volume, engine preference (`EnginePreference` enum: Auto/SpotifySdk/Open), `hide_upsell` toggle. Load failures always fall back to defaults. Exposed app-wide as `state::SETTINGS`. |
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
  feature for linux-gnu / macOS (arm64+x86_64) / windows-msvc and publishes a
  GitHub Release. Because this is a GTK/WebKit GUI, Linux is **glibc with the
  system webkit2gtk dev packages**, never musl. Web/Android/iOS are still
  Connect-only and tracked in `docs/PLATFORM_PARITY.md` (do not add them to the
  release matrix yet).## 6. Discoveries & gotchas (learned the hard way)

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
  `fetchAccessToken`). **This credentialed cross-origin fetch is UNVERIFIED** —
  no headless sandbox can confirm Spotify's CORS / third-party-cookie behavior
  for it. It's the one runtime item left before full web parity; if it fails
  live, the login gate surfaces a clear CORS error and needs a different capture
  mechanism. Do not claim web login "works" until validated in a real browser.
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
- **APK output:** `target/android/release/` (build) / Play AAB via `dx bundle
  --platform android --release` (runs `gradle bundleRelease`). `dx` signs
  nothing — sign with `apksigner` using a generated keystore for a store-ready
  APK.
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
    symlinks in the NDK bin dir (`cc-rs` needs the exact names).
  - The headless toolchain needs only env vars: `ANDROID_HOME`/`ANDROID_SDK_ROOT`
    (`$GITHUB_WORKSPACE/android-sdk`), `ANDROID_NDK_HOME` (=SDK/NDK/25.2.9519653),
    `JAVA_HOME` (JDK 17 via `actions/setup-java`), rustup
    `aarch64-linux-android`. Install via `sdkmanager`:
    `platforms;android-33`, `build-tools;33.0.2`, `ndk;25.2.9519653`.
  - Signing: `keytool -genkeypair` (throwaway keystore) + `apksigner sign` from
    `build-tools/33.0.2`; upload `*-signed.apk`.
  - `dx build --platform web --release` writes the site to the default `dist/`
    dir (dioxus's internal out-dir default — no `out_dir` set in Dioxus.toml);
    the `web` job tarballs + sha256s it.

## 7. Testing

- Unit tests are network-free and live next to the code (`#[cfg(test)]` in
  `spotify/mod.rs`, `adblock/mod.rs`, etc.).
- Run `cargo test` and `cargo test --no-default-features`. Do NOT run the app.

## 8. Updating this file

Whenever a change touches conventions, dependencies, architecture, or reveals
a gotcha, update the matching section here in the same change. The aim: the
next agent reads `AGENTS.md` + `RULES.md` and avoids the mistakes documented
above (especially around `dx serve`, the wry/dioxus versions, and CSS-in-the-binary).