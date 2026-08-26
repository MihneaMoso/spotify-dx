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
  (±`libappindicator3-dev`, `librsvg2-dev`).
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
4. Playback: `player::` — desktop uses the hidden WebView running the SDK;
   web/mobile fall back to the Spotify Connect API.
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
| `spotify/` | API models (`models.rs` — incl. `SavedTrack` envelope for `/me/tracks`), filtered HTTP client (`client.rs`), endpoints (`api.rs` — incl. `get_artist_albums`/`get_artist_related`), playback endpoint (`player_api.rs`), session helpers (`session.rs`), request store (`store.rs` — in-flight coalescing, memory TTL, disk SWR). |
| `adblock/` | Brave-style ad-block engine (`engine.rs` — `adblock` crate `Engine` on a dedicated `!Send` thread with `mpsc` channel IPC), blocklist fetch/cache (`adguard_api.rs`), cosmetic CSS scaffold (`mod.rs::cosmetic`). Facade: `should_block(url)`, `record_drop()`, `stats_snapshot()`. |
| `player/` | `mod.rs` dispatch (desktop → webview_bridge, else Connect API), `playback_sdk.rs` (embedded SDK HTML/JS), `webview_bridge.rs` (hidden WebView + IPC), `engine.rs` (`PlaybackEngine` trait). `should_use_open_engine()` checks `EnginePreference`; `play_uri()` routes to `open_play_uri()` via the streaming engine. |
| `ui/` | `router.rs` (incl. `/liked`, `/queue`, `/settings`), `theme.rs` (tokens mirrored from CSS + drift-guard tests incl. the custom-property linter), `icons.rs` (inline SVG), `components/`, `pages/`. |
| `ui/components/` | `app_layout.rs` (shell + sidebar resize), `top_bar.rs` (history/search/avatar menu), `nav.rs` (`SideNav`/`BottomNav`), `now_playing.rs` (right column), `player_bar.rs`, `progress_bar.rs`, `primitives.rs` (`SectionHeader`/`HeroHeader`/`TrackTable`/`SkeletonShelves`), `album_art.rs`, `card.rs` (MediaCard w/ `extra_class`), `track_row.rs`, `toast.rs`. |
| `ui/pages/` | `login.rs`, `home.rs`, `search.rs`, `library.rs`, `liked.rs`, `queue.rs`, `settings.rs`, `playlist.rs`, `album.rs`, `artist.rs`. |
| `media/` | `audio.rs`: symphonia decode (FLAC/M4A/MP3/OGG, seek, gapless planner) with tests. `images.rs`: disk-cached artwork loader (SHA-256 keyed, 128-file LRU, 30-day TTL). `sink.rs`: rodio audio sink thread (`MixerDeviceSink` + `Player` via `rodio::play()`), `SinkCommand` channel, `SinkState` atomics. |
| `streaming/` | Open streaming engine (Phase 4b). `provider.rs`: `Provider` trait, `Resolution` enum, `TrackQuery`. `odesli.rs`: song.link ID mapping with in-memory cache. `cache.rs`: stream-URL cache (memory + disk, 50-min TTL, FIFO 256). `resolver.rs`: cache → Odesli → provider failover. `providers/{tidal,qobuz,youtube}.rs`: TIDAL (live uptime list + fallback pool), Qobuz (ISRC search), YouTube (InnerTube API). |
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
- Cargo features: `default = ["desktop"]`; `desktop`, `web`, `mobile`.
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

## 6. Discoveries & gotchas (learned the hard way)

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
- **The keychain token is a hint, not the session.** Since the app now always
  shows the `open.spotify.com` WebView at startup (that page is the source of
  truth, and it needs the webview to keep refreshing tokens), `auth::init()` no
  longer writes `AUTH_STATE` or fast-paths past the login gate — it only
  reports whether a clock-valid token exists (used by the headless build). A
  stored token that went stale server-side (e.g. rate-limited to 429 on
  `api.spotify.com`) previously landed the app on a Home screen that spun
  forever.
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
  Origin/Referer/cookies. The web player itself now routes most browsing through
  `api-partner.spotify.com/pathfinder` (GraphQL) and `spclient.wg.spotify.com`
  instead of `api.spotify.com/v1`; neither accepted our BQA token in testing
  (401/404). Do not burn time re-testing whether the 429 has cleared — the app
  self-heals when it does.
- `auth::init()` no longer writes `AUTH_STATE` — see §6.7 note above.

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
- **Odesli API** (`api.song.link`) maps Spotify track IDs to TIDAL/Qobuz/YouTube
  IDs. No API key needed. Results cached in-memory by Spotify track ID.

## 7. Testing

- Unit tests are network-free and live next to the code (`#[cfg(test)]` in
  `spotify/mod.rs`, `adblock/mod.rs`, etc.).
- Run `cargo test` and `cargo test --no-default-features`. Do NOT run the app.

## 8. Updating this file

Whenever a change touches conventions, dependencies, architecture, or reveals
a gotcha, update the matching section here in the same change. The aim: the
next agent reads `AGENTS.md` + `RULES.md` and avoids the mistakes documented
above (especially around `dx serve`, the wry/dioxus versions, and CSS-in-the-binary).