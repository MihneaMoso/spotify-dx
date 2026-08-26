# Spotify API architecture & dual-engine playback

This document explains, for this repo (`spotify-dx`, a Rust + Dioxus desktop
client):

1. how the current Spotify API layer is structured,
2. how that API layer connects to the UI layer, and
3. how **every signed-in user** gets full-track playback — Premium accounts via
   the official Web Playback SDK, free accounts via an open multi-source engine
   (TIDAL/Qobuz/YouTube community backends).

It is written from the code in `src/` and from documented Spotify Web API /
Web Playback SDK behavior. Read `AGENTS.md`/`RULES.md` for the project-wide
rules (notably: never run `dx serve` yourself; verify with `cargo check
--features desktop`).

---

## 1. The API layer — `src/spotify/`

The whole data/back-end side of the app lives in `src/spotify/`. It is a thin,
typed wrapper over Spotify's REST Web API (`https://api.spotify.com/v1`) plus a
separate "player" API for controlling playback.

| File | Responsibility |
| --- | --- |
| `models.rs` | Serde types matching Spotify's JSON (`Track`, `Album`, `Artist`, `Playlist`, `Paged<T>`, `SearchResults`, `UserProfile`, …). `#[serde(default)]` on optional fields keeps partial responses from breaking. |
| `client.rs` | The shared `reqwest::Client` (mimics a Chrome desktop UA) + the **filtered** request helpers `filtered_get` / `filtered_get_auth` / `filtered_put_auth` / `filtered_post_auth`. Every outbound request goes through `adblock::should_block` before being sent. |
| `session.rs` | Token lifecycle: `current_token()`, `has_valid_session()`, `ensure_token()` (refresh if expired), `refresh_and_store()` (writes back into the keychain + `AUTH_STATE`). |
| `api.rs` | The high-level REST **endpoints**: `get_home`, `search`, `get_album`, `get_playlist`, `get_artist`, `get_featured_playlists`, `get_new_releases`, `get_recommendations`, `get_current_user_profile`, etc. Also `cached_get_json` / `live_get_json` (store-backed helpers) + `pipeline_load` (token lifecycle + 401-refresh + 429-backoff). |
| `player_api.rs` | The **Connect** playback endpoints (`/v1/me/player/…`): `play`, `pause`, `skip`, `seek`, `set_volume`, `get_playback_state`, plus `random_device_id`. |
| `store.rs` | Three-tier request cache: in-flight coalescing (single-flight via `watch`), memory TTL (5 min, FIFO-capped 256), disk stale-while-revalidate (24h window). `Store::global()` is the singleton; `resolve()` returns cached bytes or spawns a background `leader()` fetch. |

### 1.1 The request pipeline (`store.rs` + `api.rs`)

Every REST read funnels through `cached_get_json(url)` (cacheable) or
`live_get_json(url)` (bypasses cache, used for search):

1. `store::Store::global().resolve(url, pipeline_load)` checks the three-tier cache:
   - **In-flight coalescing**: if another caller is already fetching this URL,
     join their `watch` channel instead of issuing a duplicate request.
   - **Memory TTL** (5 min): return immediately if fresh.
   - **Disk SWR** (24h): return stale bytes immediately, spawn background
     `leader()` refresh.
2. On cache miss, `pipeline_load(url)` runs:
   - `session::ensure_token()` → returns a live access token (refreshing if the
     stored one is expired).
   - `client::filtered_get_auth()` (ad-filter gate + Bearer header).
   - `classify(resp)` maps the HTTP status:
     - `401` → `Unauthorized` (triggers a one-shot token refresh + retry).
     - `429` → `Throttled(secs)` (honours `Retry-After`, sleeps, retries).
     - `2xx` → `Success(body)`.
     - **anything else, including `403`, → `ApiError(status, body)`**.
3. Deserialize to `serde_json::Value`, write it to memory + disk cache, return.

So a `403` from a metadata endpoint surfaces as
`AppError::Spotify("403: <body>")`. Note: the pipeline has **no special
handling for `403`** — it is just passed through as a generic API error. (There
is also no `Forbidden` branch anywhere in the codebase.)

### 1.2 The player API (`player_api.rs`)

Level 2 of the API layer controls actual playback via the Spotify **Connect**
REST endpoints, all under `https://api.spotify.com/v1/me/player`:

- `play(device_id, uri, position_ms)` → `PUT /me/player/play?device_id=…`
- `pause(device_id)` → `PUT /me/player/pause?device_id=…`
- `skip(device_id, next)` → `POST /me/player/next|previous?device_id=…`
- `seek(device_id, ms)` → `PUT /me/player/seek?position_ms=…&device_id=…`
- `set_volume(device_id, pct)` → `PUT /me/player/volume?volume_percent=…&device_id=…`

Each one reads the token via `session::ensure_token()`, attaches it with
`client::filtered_*_auth(...)`, and then does a simplistic `200 | 204 => Ok`
vs. `status => AppError::Playback("play failed with {status}")` match.

**These are the calls that 403 on a non-Premium account.** They are only used
by the SDK engine path. On free accounts the app routes through the open
multi-source engine instead, which never touches the Connect API.

## 2. The playback sub-system — `src/player/`

Playback is split by engine behind a unified `PlaybackEngine` trait
(`src/player/engine.rs`):

- **desktop** (`--features desktop`, the primary target): supports two engines:
  - **SDK engine**: drives playback through the **Spotify Web Playback SDK**,
    running in a hidden 1×1 off-screen wry WebView (`webview_bridge.rs` +
    embedded HTML/JS in `playback_sdk.rs`). Premium-only.
  - **Open engine** (`src/streaming/`): resolves a `spotify:track:` URI to a
    direct audio URL via TIDAL/Qobuz/YouTube community backends, then plays it
    through a local Rust audio sink (`rodio` + `symphonia` decode). Works on
    **any account tier**.
- **web / mobile**: falls back to the **Connect API** (`player_api.rs`).

Engine selection is handled by `player::should_use_open_engine()`, which reads
`EnginePreference` from `settings.rs`:
- **Auto** (default): SDK for Premium accounts, open engine for free accounts.
- **SpotifySdk**: force SDK (Premium only; free accounts get an error).
- **Open**: force open engine regardless of account tier.

Key entry points in `player/mod.rs`, all of which the UI calls:

- `launch(uri)` — fire-and-forget async spawn of `play_uri(&uri)`, used by every
  play button (`TrackRow`, album/playlist "Play" buttons).
- `play_uri(uri)` — dispatches to either the SDK engine or the open engine
  based on `should_use_open_engine()`. The SDK path looks up
  `PLAYER_STATE.device_id` and calls `spotify::player_api::play(...)` through
  the Connect API. The open engine path resolves a stream URL and feeds it to
  the local audio sink.
- `play()` / `pause()` / `next()` / `prev()` / `seek()` / `volume()` — dispatch
  to whichever engine is active.

### 2.1 Desktop: the hidden WebView + Web Playback SDK

`webview_bridge.rs::init()` builds an off-screen WebView (1×1, pushed to
`y = -9999`, `with_visible(false)`) that loads `playback_sdk::SDK_HTML`. That
document:

- loads `https://sdk.scdn.co/spotify-player.js`,
- constructs `new Spotify.Player({ name: 'Spotify DX', getOAuthToken: … })`,
- forwards `ready` / `not_ready` / `player_state_changed` /
  `authentication_error` / `initialization_error` events to Rust over
  `window.ipc.postMessage`,
- exposes a `window._relay` object (`play`, `pause`, `next`, `prev`, `seek`,
  `volume`, `provideToken`, `connect`) that Rust calls via
  `WebView::evaluate_script`.

On `ready`, the SDK's `device_id` is written into `PLAYER_STATE.device_id`. All
subsequent "play" actions call `player_api::play(device_id, uri, …)` against the
Connect REST API.

---

## 3. How the API layer connects to the UI layer

The glue is **global Dioxus signals** in `src/state.rs` (single source of
truth) plus Dioxus `use_resource` hooks. There is no prop-drilling of network
data.

```
UI component (page)
   │  use_resource(|| async move { api::get_album(&id).await })
   ▼
src/spotify/api.rs  ──►  session::ensure_token()
   ────────────────►  client::filtered_get_auth()  ──► adblock gate
   ────────────────►  reqwest GET https://api.spotify.com/v1/…
   ────────────────►  cache, classify, deserialize into models::{...}
   ▼
returns Result<Model, AppError>
   ▼
UI renders from the resource snapshot (spinner / error banner / content)
```

Concretely:

- **Data reads** — e.g. `Home` (`ui/pages/home.rs`) calls
  `use_resource(|| async move { api::get_home().await })`; `Album` calls
  `api::get_album(&id)`, `Playlist` calls `api::get_playlist(&id)`, `Search`
  calls `api::search(...)`. The page reads `resource.read()` to decide between
  a spinner, an error banner (`{err.to_string()}`), or the rendered lists/cards.
- **Global state** — `state.rs` exposes `APP_STATE`, `PLAYER_STATE`,
  `AUTH_STATE`, `ADBLOCK_STATS`, `APP_ERROR` as `Signal::global`s. Components
  read/write these instead of threading props. `PlayerState` holds the current
  `track`, `is_playing`, `position_ms`, `duration_ms`, `volume`, and crucially
  `device_id` (the SDK-reported Connect device).
- **Playback actions** — any `TrackRow`/page play button calls
  `crate::player::launch(uri)` (or `player::play()/pause()/…` directly), which
  routes through `player/mod.rs` → `player_api.rs` / `webview_bridge.rs`. The
  SDK's `player_state_changed` events come back through the IPC queue into
  `playback_sdk::parse_sdk_state()` and are written into `PLAYER_STATE`, which
  the persistent `PlayerBar` (`ui/components/player_bar.rs`) renders (title,
  artwork, progress, play/pause, scrub, volume).
- **Auth** — `Login` (`ui/pages/login.rs`) calls `auth::login()`, writes the
  returned `AuthState` into `AUTH_STATE`, and calls `player::on_authenticated()`
  so the SDK receives its token and reconnects. `App` (`app.rs`) gates between
  `Login` and `Router::<Route>` based on `AUTH_STATE.read().is_authenticated()`.
- **Errors** — API failures bubble up as `AppError` (`app_error.rs`). Page-level
  ones are shown in a local `error-banner`; other failures are pushed through
  `state::publish_error` to the `Toast` component.

In short: **UI → `player::`/`api::`/`auth::` → `client::` (ad-filter + token) →
Spotify HTTP → models → global signals → back to UI.**

---

## 4. How free accounts get full-track playback

The app requests the `streaming` OAuth scope (`src/auth/mod.rs`, `SCOPE`). The
Web Playback SDK and Connect `/me/player` endpoints are Premium-only — so on a
free account the SDK engine cannot initialize and the Connect API returns `403
Forbidden`. The solution is the **open multi-source engine**, which bypasses
Spotify's gated stream entirely.

### 4.1 The Web Playback SDK requires Premium

Spotify's Web Playback SDK is **explicitly gated to Premium accounts**. For a
`"free"` product user, `player.connect()` fails; the SDK emits
`initialization_error: Failed to initialize player` and the device never becomes
ready. So `PLAYER_STATE.device_id` never gets populated on a free account.

### 4.2 The Connect API `/me/player` endpoints require Premium + `streaming`

Because no SDK device is active, the Connect `play` calls against
`PUT /v1/me/player/play` return **`403 FORBIDDEN`** on non-Premium tokens.
The same applies to `pause`, `skip`, `seek`, `volume`, and reading player
state — they all sit under `/v1/me/player`, which requires a Premium
subscription.

### 4.3 The open engine bypasses the Premium gate

When `should_use_open_engine()` returns `true` (free account, or user chose
`EnginePreference::Open`), the playback path never touches the Connect API:

1. `play_uri(uri)` dispatches to `src/streaming/resolver.rs`.
2. The resolver maps the Spotify track ID to provider IDs via **Odesli
   (song.link)**, then tries providers in order:
   - **TIDAL**: community Hi-Fi proxy instances with live uptime lists.
   - **Qobuz**: ISRC search + Odesli fallback.
   - **YouTube InnerTube**: audio-only fallback with PoToken timeout guard.
3. The first successful resolution returns a direct stream URL, cached in
   `src/streaming/cache.rs` (memory + disk, 50-min TTL).
4. The URL is fed to `src/media/sink.rs` (rodio + symphonia decode) for local
   playback. Metadata and artwork still come from Spotify — the UX is pure
   "Spotify" regardless of which engine plays.

### 4.4 Engine selection & automatic fallback

The `EnginePreference` setting (persisted in `settings.json`) controls which
engine is used:

| Setting | Premium account | Free account |
| --- | --- | --- |
| `Auto` (default) | SDK engine | Open engine |
| `SpotifySdk` | SDK engine | Error (no device) |
| `Open` | Open engine | Open engine |

In `Auto` mode, the selection is made after login by reading
`AuthState.product` (`"premium"` or `"free"`). The SDK path is preferred for
Premium because it offers Spotify-native features (sync across devices, native
queue). The open engine is the fallback that ensures no user is left without
playback.

### 4.5 What the app still doesn't do

- The app does **not** stream audio through Spotify's ad-supported stream. Free
  accounts get audio from third-party providers, so there are no in-stream ads
  to block or substitute.
- The `product` field is read to select the engine, but no UI elements are
  hidden or disabled based on account tier — every user sees the same player
  bar, transport controls, and now-playing view.

---

## 5. End-to-end request/playback sequence (quick reference)

1. `Login` → OAuth PKCE (`auth::login`) → token + `streaming` scope in `AUTH_STATE`.
   Profile fetch populates `AuthState.product` (`"premium"` / `"free"`).
2. `App` sees `is_authenticated()` → renders `Router::<Route>` shell.
3. Page mounts → `use_resource(api::get_* )` → `session::ensure_token()` →
   `client::filtered_get_auth` (ad-filter) → REST → models → renders.
4. User presses play → `player::launch(uri)` → `play_uri`:
   - **Premium + SDK engine**: needs `PLAYER_STATE.device_id` (from Web Playback
     SDK) → `player_api::play` → `PUT /v1/me/player/play` → plays via Spotify
     Connect.
   - **Free or Open engine**: resolves `spotify:track:` → Odesli → TIDAL/Qobuz/
     YouTube → direct stream URL (cached) → local rodio+symphonia decode → plays
     locally. Metadata/artwork still from Spotify.

For a full architectural map (module layout, router, features, gotchas), see
`RULES.md` §4 and §6.

