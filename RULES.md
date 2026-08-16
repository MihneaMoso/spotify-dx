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

- **`SPOTIFY_CLIENT_ID` is required at compile time.** It is read via
  `env!("SPOTIFY_CLIENT_ID")` in `src/auth/mod.rs:19`. Without it in the
  environment, every build/check fails with "environment variable not defined".
  If a build fails for that reason, that's the first thing to check.
- Linux desktop builds need `pkg-config`, `libwebkit2gtk-4.1-dev`
  (±`libappindicator3-dev`, `librsvg2-dev`).
- The dev loop: user runs `dx serve` (dx CLI 0.7.x). It is **auto-reload**, so
  code changes are picked up without a restart.

## 4. Project agent-map

### 4.1 What it is

A cross-platform Spotify client ("Spotify DX") written in Rust with Dioxus.
Its headline trick: on **premium** accounts it plays full tracks via the Web
Playback SDK driven from a hidden wry WebView, and on **free** accounts it
blocks Spotify's ad pipeline + "premium preview" 30-second interstitial by
routing the SDK's network traffic through an in-process ad-blocker.

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
| `auth/` | PKCE OAuth (`pkce.rs`), keychain persistence (`token_store.rs`), refresh/init flows (`mod.rs`). `CALLBACK_PORT` fixed at 8888 — Spotify dashboard only accepts concrete redirect URIs. |
| `spotify/` | API models (`models.rs`), filtered HTTP client (`client.rs`), endpoints (`api.rs`), playback endpoint (`player_api.rs`), session helpers (`session.rs`), request cache (`cache.rs`). |
| `adblock/` | AdGuard DNS-filter parsing (`adguard_api.rs`), radix-trie blocklist + DoH resolver (`dns_filter.rs`), `mod.rs` facade. |
| `player/` | `mod.rs` dispatch (desktop → webview_bridge, else Connect API), `playback_sdk.rs` (embedded SDK HTML/JS), `webview_bridge.rs` (hidden WebView + IPC). |
| `ui/` | `router.rs`, `theme.rs` (design tokens mirrored in CSS), `icons.rs` (inline SVG), `components/`, `pages/`. |
| `ui/components/` | `app_layout.rs` (shell + sidebar resize), `nav.rs` (`SideNav`/`BottomNav`), `player_bar.rs`, `progress_bar.rs`, `album_art.rs`, `card.rs`, `track_row.rs`, `toast.rs`. |
| `ui/pages/` | `login.rs`, `home.rs`, `search.rs`, `library.rs`, `playlist.rs`, `album.rs`, `artist.rs`. |
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

### 6.3 The bottom-nav / player-bar stack (current layout)

`.app-shell` is a 3-row grid on desktop:
`sidenav main / player player / nav nav` — the bottom nav is a real grid row at
the very bottom, the player bar sits directly above it. The `@media
(max-width: 820px)` block swaps the side nav for the same bottom nav and stacks
`main / player / nav`. When changing this:
- Keep the grid-area assignments (`side-nav`, `main-content`, `player-bar`,
  `bottom-nav`) in `assets/main.css` section 3 consistent.
- `.sidebar-resizer` is `position: fixed` and its `bottom` must equal
  `calc(var(--player-height) + var(--bottom-nav-height))` to span the full
  rail height.
- `.toast` floats above the player bar and must also clear the bottom-nav row.

### 6.4 Desktop window & "wide vs narrow" media queries

The desktop window defaults to 1200×780. That is WIDER than the
`max-width: 820px` breakpoint, so base (non-media-query) CSS governs most
desktop layout. When a "mobile-only" rule (e.g. `.bottom-nav`) is involved,
verify which rule actually wins in the base sheet — a later same-specificity
rule overrides an earlier `display: none`.

### 6.5 WebView / SDK pitfalls

- The hidden WebView runs the Web Playback SDK; on free accounts the SDK
  reports `init_error: Failed to initialize player` — that warning in the dx
  log is expected/benign.
- `i.scdn.co` artwork must never be blocked; `*.spotifycdn.com` ad/preview
  gates are the ones the blocklist targets.
- No direct dioxus signal access from the wry IPC handler — queue and drain.

### 6.6 Auth specifics

- `SPOTIFY_CLIENT_ID` compile-time (`env!`); OAuth redirect is
  `http://127.0.0.1:8888/callback` (fixed port — Spotify requires concrete
  URIs). Mobile deep-link scheme `spotifydx://callback`.
- Session is persisted in the OS keychain (`keyring`), not on disk.

## 7. Testing

- Unit tests are network-free and live next to the code (`#[cfg(test)]` in
  `spotify/mod.rs`, `adblock/mod.rs`, etc.).
- Run `cargo test` and `cargo test --no-default-features`. Do NOT run the app.

## 8. Updating this file

Whenever a change touches conventions, dependencies, architecture, or reveals
a gotcha, update the matching section here in the same change. The aim: the
next agent reads `AGENTS.md` + `RULES.md` and avoids the mistakes documented
above (especially around `dx serve`, the wry/dioxus versions, and CSS-in-the-binary).