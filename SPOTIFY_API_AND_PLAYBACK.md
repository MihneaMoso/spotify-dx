# Spotify API architecture & why playback 403s on a non-Premium account

This document explains, for this repo (`spotify-dx`, a Rust + Dioxus desktop
client):

1. how the current Spotify API layer is structured,
2. how that API layer connects to the UI layer, and
3. why a user whose Spotify dev account **does not have Premium** can't play
   (and, in practice, can't meaningfully drive) any music — the Spotify API
   returns **`403 Forbidden`**.

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
| `api.rs` | The high-level REST **endpoints**: `get_home`, `search`, `get_album`, `get_playlist`, `get_artist`, `get_featured_playlists`, `get_new_releases`, `get_recommendations`, `get_current_user_profile`, etc. Also `api_get_json` (central helper) + `classify()` for status handling + `fetch_page`/`get_object` generic helpers. |
| `player_api.rs` | The **Connect** playback endpoints (`/v1/me/player/…`): `play`, `pause`, `skip`, `seek`, `set_volume`, `get_playback_state`, plus `random_device_id`. |
| `cache.rs` | In-memory 5-minute TTL cache keyed by request URL, persisted to disk under the cache dir for offline reads. |

### 1.1 The request pipeline (`api.rs::api_get_json`)

Every REST read funnels through `api_get_json(url, cacheable)`:

1. `session::ensure_token()` → returns a live access token (refreshing if the
   stored one is expired).
2. Try the in-memory/on-disk cache (`cache::get`) and return early on a hit.
3. `request_once(url, token)` → `client::filtered_get_auth()` (ad-filter gate +
   Bearer header) → `classify(resp)`.
4. `classify()` maps the HTTP status:
   - `401` → `Unauthorized` (triggers a one-shot token refresh + retry).
   - `429` → `Throttled(secs)` (honours `Retry-After`, sleeps, retries).
   - `2xx` → `Success(body)`.
   - **anything else, including `403`, → `ApiError(status, body)`**.
5. Deserialize to `serde_json::Value`, write it to the cache, return.

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

**These are the calls that 403 on a non-Premium account** (see §3 below).

## 2. The playback sub-system — `src/player/`

Playback is split by renderer in `player/mod.rs`:

- **desktop** (`--features desktop`, the primary target): drives playback through
  the **Spotify Web Playback SDK**, running in a hidden 1×1 off-screen wry
  WebView (`webview_bridge.rs` + embedded HTML/JS in `playback_sdk.rs`).
- **web / mobile**: falls back to the **Connect API** (`player_api.rs`).

Key entry points in `player/mod.rs`, all of which the UI calls:

- `launch(uri)` — fire-and-forget async spawn of `play_uri(&uri)`, used by every
  play button (`TrackRow`, album/playlist "Play" buttons).
- `play_uri(uri)` — looks up the current `PLAYER_STATE.device_id`, then calls
  `spotify::player_api::play(...)` **through the Connect API** regardless of
  renderer (the device was reported by the SDK). This is the path that 403s.
- `play()` / `pause()` / `next()` / `prev()` / `seek()` / `volume()` — dispatch
  to the SDK bridge on desktop, or to `player_api` on other renderers.

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

## 4. Why playback 403s on a non-Premium account

This is the important part. The app requests the `streaming` OAuth scope
(`src/auth/mod.rs`, `SCOPE`), tries to initialise the Web Playback SDK, and then
issues Connect `play` calls. **Every one of those playback steps is
Premium-only** in Spotify's model, so the API answers `403 Forbidden`.

### 4.1 The Web Playback SDK requires Premium

Spotify's Web Playback SDK is **explicitly gated to Premium accounts**. For a
`"free"` product user, `player.connect()` fails; the SDK emits
`initialization_error: Failed to initialize player` (the "benign" warning the
log notes in `RULES.md §6.5`) and the device never becomes ready. When the
hidden WebView first boots before the user logs in, `connect()` also dies with
an auth error — `reconnect()` (called after login) is supposed to fix that, but
it cannot overcome the underlying Premium gate. So `PLAYER_STATE.device_id`
never gets populated on a free account.

### 4.2 The Connect API `/me/player` endpoints require Premium + `streaming`

Because no SDK device is active, the UI's play requests go
`launch(uri)` → `player::play_uri` → `spotify::player_api::play(device, uri)`
against `PUT /v1/me/player/play`. Spotify REST returns **`403 FORBIDDEN`** for
these control endpoints on non-Premium tokens. The same applies to `pause`,
`skip`, `seek`, `volume`, and to reading player state — they all sit under
`/v1/me/player`, which requires a Premium subscription (the Web API docs state
the "Playback" endpoints and the `streaming` scope are Premium-only).

Because `device_id` is `None`, `play_uri` actually short-circuits first with
`AppError::Playback("no playback device available yet")` — and once a device id
is ever reported, the follow-up `play` call returns 403. Either way, nothing
plays.

### 4.3 Nothing in the app checks `product`

The `/v1/me` profile response carries a `product` field (`"premium"` /
`"free"`), and `models.rs::UserProfile` does deserialize it (`pub product:
Option<String>`). **But nothing reads or gates on it** — the codebase has no
`premium`, `product`, or `403` special-casing. The app always attempts the
Premium-only path and just surfaces the resulting error. That's why the failure
appears as a generic Spotify/playback error rather than a friendly
"this needs Premium" message.

### 4.4 Why it can feel like "can't view any music either"

Strictly, *browsing metadata* (search, albums, playlists, artist, featured
playlists, new releases) does **not** require Premium and works fine on a free
token. However, several things make it look like "everything 403s":

- The `streaming` scope and `/me/player` path are the true Premium gates; they
  are the ones returning 403.
- Control endpoints return a terse 403 — e.g.
  `AppError::Playback("play failed with 403")` — with no hint about Premium.
- If a dev account's token is also missing non-playback scopes (or the app is
  not granted the needed scopes in the Spotify dashboard for that client id),
  even metadata calls can 403 on **authorization/scope** grounds — a separate
  cause from the Premium gate, but with the same status code.

For **this** dev account, the dominant blocker is the Premium requirement on
the Web Playback SDK + Connect playback endpoints. Free accounts simply cannot
stream full tracks with this stack.

### 4.5 Suggested (documentation-only) takeaways

- Detect the free/premium case up front by reading `UserProfile.product` after
  login and disable/hide playback controls with a "Premium required" message
  instead of waiting for a 403.
- Treat `403` from `/me/player*` distinctly from other `403`s (scope vs.
  Premium) so the UI can say why.
- Metadata browsing (search/library/home) is not Premium-gated; if those also
  return 403, inspect the granted scopes in the Spotify Developer Dashboard for
  that client id.

---

## 5. End-to-end request/playback sequence (quick reference)

1. `Login` → OAuth PKCE (`auth::login`) → token + `streaming` scope in `AUTH_STATE`.
2. `App` sees `is_authenticated()` → renders `Router::<Route>` shell.
3. Page mounts → `use_resource(api::get_* )` → `session::ensure_token()` →
   `client::filtered_get_auth` (ad-filter) → REST → models → renders.
4. User presses play → `player::launch(uri)` → `play_uri` → needs
   `PLAYER_STATE.device_id` (from Web Playback SDK) → `player_api::play` →
   `PUT /v1/me/player/play` → **403 on a non-Premium account** →
   `AppError::Playback` → shown in the bar / consumed silently by the
   fire-and-forget `launch`.

For a full architectural map (module layout, router, features, gotchas), see
`RULES.md` §4 and §6.

