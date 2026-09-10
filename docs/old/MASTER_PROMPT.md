# MASTER PROMPT — Spotify DX: replicate open.spotify.com end-to-end

> Give this entire document to a coding agent (a fresh context) as its only
> instruction set. Read `AGENTS.md` and `RULES.md` first — they are binding.
> This prompt is the *spec*; RULES.md/AGENTS.md are the *constraints*.

---

## 0. Role & ground rules (non-negotiable)

You are the senior engineer refactoring **Spotify DX** (a Rust + Dioxus 0.7
desktop Spotify client, `wry 0.53`, `dx` CLI 0.7.x). You will make the app
**behave exactly like the open.spotify.com web app**, while keeping the AdGuard
ad-filter and fast native (RSX) components.

Before anything else:

1. Read `AGENTS.md` and `RULES.md` **fully** and follow them. The most
   important rules are repeated here:
   - **NEVER run, stop, restart, or kill `dx serve` or the app binary.** The
     user runs the dev loop manually with auto-reload. You verify with:
     `cargo check --features desktop`, `cargo clippy --features desktop -- -D warnings`,
     `cargo test`, `cargo test --no-default-features`. Never `cargo run`.
   - Do not touch `target/`.
   - **No `unsafe` code anywhere** (`#![forbid(unsafe_code)]` in `main.rs` is a
     hard invariant — the gtk-rs 0.18 `present()`/`close()` methods are safe;
     keep it that way).
   - No code comments unless they explain *why*. Match the existing style.
   - Keep `RULES.md` up to date in the same change whenever you discover a
     gotcha or change architecture.
   - Never commit unless explicitly asked.
2. Before writing code, read the module map and key files listed in §3 so you
   know what already exists. A lot of the plumbing is **already done** — do not
   re-architect working parts.

---

## 1. Context — where the project is right now

The project was migrated from the official Spotify Web API (OAuth PKCE +
`SPOTIFY_CLIENT_ID`) to a **web-session** architecture, because the developer
API returns `403 Forbidden` on a non-Premium account (the Web Playback SDK and
the `/v1/me/player` Connect endpoints are Premium-only). The previous refactor
left the app stuck at a static "Starting app…" screen — **that is now fixed**:
`src/auth/webview_login.rs` opens a real GTK window hosting `open.spotify.com`,
captures the web-player session, and the app then renders the native UI.

Current working state (verified: `cargo check`, `clippy -D warnings` and all 14
unit tests pass):

- **Login**: first launch opens a GTK window (wry WebView) at `open.spotify.com`.
  Injected JS polls `open.spotify.com/get_access_token?reason=transport&productType=web_player`
  (requires the HttpOnly `sp_dc` cookie) and reports the access token to Rust
  over `window.ipc.postMessage` → `auth::login()` → `on_session_captured()` →
  `AUTH_STATE` flips → `App` swaps from `Login` to `Router`.
- **Session persistence**: session cookies live in the WebContext data dir
  (`auth::webview_data_dir()` → `data_local_dir/spotify-dx/webview_session/`),
  which is what keeps the user logged in across restarts, exactly like
  open.spotify.com. The short-lived access token (~1h) is mirrored to the OS
  keychain (`auth::token_store`) for fast boot restore. Refreshes go through the
  hidden SDK WebView (`spotify::session::ensure_token` →
  `player::webview_bridge::request_token_refresh`).
- **Metadata browsing**: `src/spotify/` (models, filtered client, api, cache,
  player_api, session) already serves Home/Search/Library/Album/Artist/Playlist
  through the web-player access token.
- **Playback (desktop)**: `src/player/` — a hidden wry WebView runs the
  **Web Playback SDK** (`playback_sdk.rs`), sharing the login WebContext so it
  inherits the session cookies. `player/mod.rs` dispatches play/pause/next/prev/
  seek/volume either to the SDK bridge or to the Connect API.
- **Free-tier gating**: `player/mod.rs::play_uri` and `player_api.rs` currently
  **hard-block playback on free accounts** with `AppError::PremiumRequired`. See
  §2 — this must change.
- **Ad-block**: `src/adblock/` (AdGuard DNS-list parsing → radix trie → DoH
  resolver). Every outbound request goes through `spotify::client::filtered_*`
  which consults the trie. Note: `dns_filter.rs` currently **whitelists all of
  `*.spotify.com`, `*.spotifycdn.com`, `*.scdn.co`** so login/API/streams are
  never blocked. §5E revisits this for free-tier ad blocking.

---

## 2. The problem you are solving

The end goal is: **the app must do everything open.spotify.com does, for the
same account, on every tier — without the official API's Premium 403s.** This is
now achieved: free accounts get full-track playback through an open multi-source
engine (TIDAL/Qobuz/YouTube community backends), while Premium accounts use the
official Web Playback SDK.

- The Web Playback SDK is Premium-only, so it cannot be the playback engine for
  free accounts. The open.spotify.com web player **does not use the SDK** — it
  uses Spotify's internal web-player playback stack.
- On a **premium** account, the app should feel and behave like the web player:
  full playback, no artificial gates.

Your job: choose and implement a playback engine that gives **parity with the
open.spotify.com web player on both tiers**, mirror its state into the native
UI, keep the AdGuard ad-filter (and use it to strip the audio ads a free tier
would otherwise hear), and finish the UX details so it "feels like
open.spotify.com" while staying a fast native app.

---

## 3. Module map (what exists today — do not re-architect working parts)

| Path | Responsibility |
| --- | --- |
| `main.rs` | Per-renderer entry points + `bootstrap()` (ad-block init + `auth::init`), window config (frameless 1200×780). |
| `app.rs` | Root `App`: stylesheet, login gate vs `Router::<Route>`, boots `player::init()` once a session exists. |
| `state.rs` | Global `Signal`s: `APP_STATE`, `PLAYER_STATE`, `AUTH_STATE`, `ADBLOCK_STATS`, `APP_ERROR`. `AuthState` (token, expiry, user_id, product premium/free, is_authenticated), `PlayerState` (track, queue, is_playing, position_ms, duration_ms, volume, shuffle, repeat, device_id). |
| `app_error.rs` | `AppError` variants (Auth, Network, AdBlock, Playback, PremiumRequired, SessionExpired, Forbidden, Spotify, Webview). |
| `auth/` | `webview_login.rs` (GTK sign-in window + token capture — **done**), `token_store.rs` (keychain + file fallback), `mod.rs` (`init`, `login`, `await_session`, `on_session_captured`, `logout`, `refresh_profile`). |
| `spotify/` | `models.rs`, `client.rs` (filtered HTTP, ad-gate), `api.rs` (metadata endpoints), `player_api.rs` (Connect REST, Premium-gated), `session.rs` (token lifecycle), `cache.rs`. |
| `player/` | `mod.rs` (dispatch + `launch`/`play_uri`), `playback_sdk.rs` (SDK HTML/JS + `SdkState` parsing), `webview_bridge.rs` (hidden WebView + IPC queue + token refresh). |
| `adblock/` | `adguard_api.rs`, `dns_filter.rs` (trie, whitelist, DoH resolve), `mod.rs` facade. |
| `ui/` | `router.rs` (routes), `theme.rs`, `icons.rs`, `components/` (app_layout, nav, player_bar, progress_bar, album_art, card, track_row, toast), `pages/` (login, home, search, library, playlist, album, artist). |

Read at minimum: `RULES.md` §4 (agent-map), §5 (conventions), §6 (gotchas);
`src/app.rs`, `src/state.rs`, `src/player/mod.rs`, `src/player/webview_bridge.rs`,
`src/auth/mod.rs`, `src/auth/webview_login.rs`, `src/spotify/session.rs`,
`src/spotify/client.rs`, `SPOTIFY_API_AND_PLAYBACK.md`.

---

## 4. Target architecture

Keep the layered shape below. Only the **playback engine** block is new/under
rework; everything else is finishing.

```
┌─────────────────────────────────────────────────────────────┐
│ Dioxus native UI (RSX components, existing)                 │
│  Router + pages + PlayerBar ← mirrors PLAYER_STATE           │
└───────────────▲───────────────────────────┬─────────────────┘
                │ reads/writes              │ user actions
┌───────────────┴───────────────────────────▼─────────────────┐
│ state.rs (global signals)   +  spotify/ (metadata, session) │
└───────────────▲───────────────────────────┬─────────────────┘
                │ engine events             │ commands
┌───────────────┴───────────────────────────▼─────────────────┐
│ PLAYER ENGINE (the work item)                               │
│  Desktop: hidden WebView hosting the real open.spotify.com  │
│  web player (SDK for Premium, open engine for free accounts)│
│  + IPC: state_changed, token_refresh, device_id             │
└───────────────▲─────────────────────────────────────────────┘
                │ shared WebContext (session cookies)
┌───────────────┴─────────────────────────────────────────────┐
│ auth/webview_login.rs (sign-in) + adblock trie (AdGuard)     │
└─────────────────────────────────────────────────────────────┘
```

### 4.1 The playback engine (the hard part — choose ONE, justify, implement)

Goal: play real audio on **both** tiers exactly like the open.spotify.com web
player, and surface track/progress/queue events into `PLAYER_STATE`.

**Option A — Drive the real open.spotify.com web player (recommended).**
A hidden/offscreen WebView loads `https://open.spotify.com` with the shared
WebContext (already logged in). Rust drives it exactly like a user would — set
the URL/queue, call play/pause/next/prev/seek/volume via `evaluate_script`, and
listen to player-state changes (Spotify's web player exposes its own state
internally; the page's UI already updates in real time — mirror it via a polling
script or DOM events into `window.ipc.postMessage`). Playback is whatever the
real web player gives you: **works on free accounts (shuffle/skips/ads) and
premium accounts (full)**. No API 403s, no SDK.
- Pros: exact parity with open.spotify.com on every tier; zero Premium gating;
  ad stream is inside the WebView so AdGuard can target it (§5E).
- Cons: it is a real web app — reverse-engineering how to reliably drive it
  (URLs, element hooks, or its internal JS API) takes research; the page may
  change. Use `with_visible(false)` + off-screen bounds + the same
  `thread_local!` + IPC-queue pattern as `webview_bridge.rs`. Do **not** run it
  in the visible main window.

**Option B — Web Playback SDK for premium + web-player fallback for free.**
Keep the existing SDK bridge for premium; for free accounts drive the web
player instead (Option A). More code paths, more divergence, but each path is
simpler.
- Pros: premium path already exists and is proven.
- Cons: two engines to maintain; free-tier still needs the web-player driver.

**Decision criteria:** pick the option that reliably plays on the *free*
account first (that is the current blocker), then unify premium on the same
path if it gives identical behavior. Whatever you choose, **define the engine
trait** (play/pause/next/prev/seek/volume/shuffle/repeat/get-state/connect)
with the existing `player/mod.rs` as the adapter layer, so the UI never touches
engine internals.

**Free-tier specifics to replicate from the web player:** shuffle-only playback,
limited skips, ads (see §5E). Do **not** show `PremiumRequired` when the web
player can actually play — that gate was a stopgap and must be removed unless
the engine genuinely cannot play.

### 4.2 Auth & session (mostly done — verify, harden)

- Verify `webview_login.rs` flow end-to-end (see §6 risks).
- `auth::init()` currently restores from keychain only when the token is valid
  (`expires_ms > now + 60s`). If the token is expired but cookies exist, the app
  shows the login window again — that window should auto-resolve almost
  instantly because the shared WebContext already has the session cookies
  (`open.spotify.com` loads logged-in and the poller fires immediately). **Test
  and confirm this "invisible re-login" is seamless**; if the login window
  flashes unnecessarily, suppress it (e.g., first try a silent refresh via the
  SDK WebView before opening the sign-in window).
- Persist more of the profile (`user_id`, display name, avatar, `product`) on
  login — `refresh_profile()` already folds `/v1/me` into `AUTH_STATE`. Make
  sure the Login page never shows once a session exists.

### 4.3 Native UI parity (finish)

The web app has these flows the native UI should cover:
- **Player bar** (`ui/components/player_bar.rs`): artwork, title/artists, live
  progress + scrub, play/pause, next/prev, shuffle/repeat, volume. Mirror every
  engine `state_changed` event into `PLAYER_STATE` and dispatch bar actions to
  the engine.
- **Queue**: the web player exposes its queue (`player_state_changed` →
  `track_window.next_tracks`, or the engine's queue API). Surface a Queue view
  (or at least populate `PLAYER_STATE.queue`) and a "Play next / Play later"
  action.
- **Context menus / actions** on tracks and cards: Play, Play next, Add to
  Liked Songs (`PUT /v1/me/tracks`), "Go to album/artist", Add to playlist.
- **Search**: debounced, with track/album/artist/playlist tabs (partially done).
- **Library**: Liked Songs, saved albums/playlists, recently played.
- **Now-playing / detail** pages for album and artist (partially done) — make
  navigation feel like the web app.
- **Empty/error/loading states** already exist; reuse them. Keep the design
  system (`theme.rs` + `assets/main.css`) — do not redesign.
- Keep the ad-block stats panel/`ADBLOCK_STATS` if present.

### 4.4 Networking & session

- `spotify::session::ensure_token` refreshes through the hidden WebView. When
  the engine is the web player (Option A), it can fetch its own token from
  `get_access_token` like the SDK does — keep `AUTH_STATE`/keychain in sync via
  the existing `token_refresh` IPC messages.
- Keep every outbound `reqwest` request behind `client::filtered_*` (the ad
  gate) — metadata calls from the native UI must keep working on a free account.

---

## 5. Workstreams (implement in this order)

### A. Lock the auth/session flow (0.5 day)
1. Re-read `src/auth/*`. Confirm `login.rs` opens the GTK window, captures the
   token, persists to keychain, flips `AUTH_STATE`, and that `App` then mounts
   the router (the swap is reactive on `is_authenticated` — don't break it).
2. Make the expired-token restart seamless (§4.2): try a silent refresh before
   opening the sign-in window.
3. Add a "Sign out" path that clears cookies + keychain + `AUTH_STATE` and
   returns to the login gate (a new webview with a clean `WebContext` — see
   `webview_bridge::shutdown` for the existing teardown pattern).

### B. Playback engine (2–4 days, the core)
1. Decide Option A vs B (§4.1) after a short spike: load `open.spotify.com`
   in an off-screen WebView (reuse `webview_bridge.rs` patterns) and find a
   reliable way to (a) detect "player ready", (b) receive track/progress
   changes, (c) send play/pause/next/prev/seek/volume/shuffle/repeat.
   Document what you found in `RULES.md`.
2. Define the engine interface in `player/mod.rs` (or a new `player/engine.rs`)
   and re-point `play_uri`, `play`, `pause`, `next`, `prev`, `seek`, `volume`,
   `shuffle`, `repeat` at it.
3. Remove the blanket `PremiumRequired` gate on `play_uri` and `player_api.rs`
   only where the engine actually plays. Keep it only for genuinely
   Premium-only features (e.g., if the web player refuses something).
4. Keep the IPC queue + dioxus-task drain pattern (webkit threads cannot touch
   dioxus signals — `RULES.md §6.5`).

### C. Player-state mirroring (1 day)
- Wire engine events → `PLAYER_STATE` (track, is_playing, position_ms,
  duration_ms, shuffle, repeat, queue, device_id) so the existing `PlayerBar`
  and pages just work. Fix any page that assumed `device_id` comes from the SDK.
- Implement live position ticking (the web player reports position; don't
  re-implement a timer if the engine streams it — else tick in the UI).

### D. Web-app feature parity in the UI (1–2 days)
- Queue view + add-to-queue actions.
- Add-to-Liked (`PUT /v1/me/tracks`), remove-from-liked, and show liked state.
- Context menus on `TrackRow`/cards (Play next, Add to queue/playlist, Go to
  album/artist, Copy link, Share).
- Confirm Home/Search/Library match the web app's content surfaces.

### E. Ad-blocking for the free tier (1–2 days, research-heavy)
The web player on free accounts injects audio ads and "ad video" interstitials
served from Spotify CDNs. The goal (user requirement): **the app keeps the
AdGuard ad-filter and free accounts hear no ads**, while real music still plays.
- Current `dns_filter.rs` whitelists all `*.spotifycdn.com` — this both protects
  music streams and lets ads through. You must find the **specific ad request
  signatures** the web player uses (ad segment URLs, `audio_ads` endpoints,
  interstitial domains) and block *those* precisely without blocking music.
  Research the web player's ad flow (network tab capture of a free-account
  session) before writing rules. Prefer URL-pattern blocks over whole-domain
  blocks.
- If audio-ad stripping proves infeasible on this timescale, deliver a
  *documented* design + the mechanism (a `player::ads` hook that mutes/skips
  ad segments the moment the engine reports an ad in `player_state_changed`) as
  a stretch goal — never ship a half-working CDN block that breaks music.
- Update the `dns_filter.rs` whitelist comment and `RULES.md §6.5` accordingly.

### F. Polish, errors, docs (0.5–1 day)
- `PremiumRequired`/`SessionExpired`/`Forbidden` surfaced as toasts exactly
  where needed; no raw error dumps in the UI.
- Update `README.md`, `RULES.md` (module map, gotchas), and this doc's status
  as you go.

---

## 6. Risks & gotchas (read carefully)

- **Do not touch `webview_bridge.rs`'s thread model.** wry/webkit must be
  touched only on the UI thread; direct signal access from IPC handlers panics.
- **`build_as_child` is X11-only.** The login window uses `build_gtk` for
  Wayland — keep that pattern for any new WebView.
- **Two WebContexts on one cookie store must never coexist.** Tear down the
  login window (`auth::webview_login::close`) before the engine/SDK WebView is
  created; otherwise webkit may corrupt/lock the cookie store.
- **The web player is a moving target.** Whatever DOM/JS hooks you rely on,
  isolate them in one file and add a "engine probe" log on boot so breakage is
  diagnosable.
- **`asset!()` compiles CSS into the binary**; editing `assets/main.css` alone
  doesn't change a running binary (the user's `dx serve` rebuilds on code
  change, but don't rely on CSS-only fixes).
- **Cargo pins:** `dioxus 0.7`, `wry 0.53` (must stay 0.53.x — dioxus-desktop
  0.7.10 already depends on it), `gtk 0.18` (Linux, optional, enabled by the
  `desktop` feature). Don't bump versions.
- **Free-account playback is the acceptance criterion.** If the engine plays on
  a free account with ads stripped, the refactor is a success. Premium-only
  features must still work for premium accounts.
- **Never open a second event loop**, never run GTK on a non-main thread,
  never `cargo run`.

---

## 7. Verification & acceptance criteria

Before finishing, run and confirm **all** of these pass (the app itself is run
only by the user, never by you):

1. `cargo check --features desktop` — clean.
2. `cargo clippy --features desktop -- -D warnings` — zero warnings.
3. `cargo test` and `cargo test --no-default-features` — all pass.
4. `#![forbid(unsafe_code)]` still holds (no `unsafe` anywhere in `src/`).
5. First-run UX: no session → sign-in window opens → login → native UI renders
   → restart → **still logged in, no login window** (or at most a flicker).
6. Free account: a track actually plays (shuffle) from a playlist/album/search;
   skip/seek/volume/shuffle/repeat work; **no audio ads**; no `PremiumRequired`
   toast blocking playback.
7. Premium account: full playback with all controls; same as web app.
8. Player bar mirrors track/progress live; queue view populated; add-to-liked
   works and persists.
9. AdGuard trie still blocks a known third-party ad host and never blocks
   `api.spotify.com`/`open.spotify.com`/`i.scdn.co`.
10. `RULES.md`/`README.md` updated to match the final architecture.

---

## 8. References

- `RULES.md`, `AGENTS.md`, `SPOTIFY_API_AND_PLAYBACK.md` (in-repo).
- Spotify Web Playback SDK docs: `https://developer.spotify.com/documentation/web-playback-sdk`
- The hidden WebView + IPC pattern to reuse: `src/player/webview_bridge.rs`.
