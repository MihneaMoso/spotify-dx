# Spotify DX — Architecture & App Flow

A comprehensive, implementation-agnostic design document. It describes **what
the system is, how requests and screens flow through it, and how it is
architected for maximum performance** — deliberately without reference to any
programming language, UI framework, or library. Component names below are
architectural roles, not code identifiers.

---

## Table of contents

1. [Product definition](#1-product-definition)
2. [Design principles](#2-design-principles)
3. [System map](#3-system-map)
4. [Application lifecycle](#4-application-lifecycle)
5. [Authentication and session architecture](#5-authentication-and-session-architecture)
6. [Global state architecture](#6-global-state-architecture)
7. [Data layer](#7-data-layer)
8. [Ad-blocking subsystem](#8-ad-blocking-subsystem)
9. [Playback architecture](#9-playback-architecture)
10. [User-interface architecture](#10-user-interface-architecture)
11. [Platform strategy](#11-platform-strategy)
12. [Self-update system](#12-self-update-system)
13. [Settings, profile, and personalization](#13-settings-profile-and-personalization)
14. [Performance architecture](#14-performance-architecture)
15. [Error handling and resilience](#15-error-handling-and-resilience)
16. [Testing and quality gates](#16-testing-and-quality-gates)
17. [Release and distribution](#17-release-and-distribution)
18. [Known limitations and open work](#18-known-limitations-and-open-work)

---

## 1. Product definition

Spotify DX is a cross-platform Spotify client that reproduces the
open.spotify.com experience as a fast native application. Its defining
properties:

- **Real Spotify sign-in.** The user logs in through Spotify's own login
  pages hosted inside the app — password, two-factor, and passkeys all work
  because it *is* the Spotify login, not a reimplementation. No developer
  application, client ID, or OAuth redirect configuration is required.
- **No paywall on playback.** Premium accounts play through Spotify's official
  Web Playback SDK. Free accounts play full tracks through an open
  multi-source streaming engine. Every signed-in user gets full-track
  playback, identical transport controls, and identical UI regardless of tier.
- **Native interface.** All screens are rendered as native interface
  components (not a wrapped website): a persistent shell with side
  navigation, top bar, player bar, optional now-playing column, and routed
  content pages.
- **Built-in ad and tracker filtering.** An in-process filtering engine drops
  third-party advertising and analytics requests before they reach the
  network, on every platform.
- **Cross-platform from one codebase.** Desktop, mobile, and web builds share
  the UI, the data layer, and the playback logic; only genuinely
  platform-specific concerns (storage backends, audio output, login-page
  hosting, packaging) diverge behind narrow seams.

---

## 2. Design principles

1. **Reuse Spotify's own session instead of owning credentials.** The app never
   sees passwords and stores no refresh token; the login session lives in
   browser cookies exactly as on the website, and the short-lived access token
   is captured from the web player's own traffic.
2. **Read from the cheapest source that stays correct.** Spotify's internal
   data API (the same one the website uses) replaces the public rate-limited
   API for almost all reads; results are cached in memory, on disk, and
   coalesced across concurrent callers.
3. **Never pay for idle.** Nothing polls on a timer when there is no work:
   background tasks sleep on event notifications, hidden pages are parked, and
   state writes happen only when values actually change.
4. **Optimistic interface, reconciled state.** Controls respond instantly from
   local state; backend confirmations reconcile afterwards.
5. **Fail fast, heal automatically.** Rate limits and expired sessions degrade
   to informative banners with automatic retry rather than endless spinners or
   dead ends.
6. **One source of truth per concern.** A small set of global observable
   stores replaces prop drilling; every screen reads the same state.
7. **Platform seams, not platform forks.** Shared logic is written once;
   storage, audio output, login hosting, and packaging diverge through small
   adapter modules.

---

## 3. System map

```
┌──────────────────────────── Application window ────────────────────────────┐
│  Native interface (declarative components, themed)                         │
│  ┌──────────────────────────────────────────────────────────────────────┐  │
│  │ App shell: top bar │ side nav │ main content │ now-playing │ player │  │
│  └──────────────────────────────────────────────────────────────────────┘  │
│  Routes: Home · Search · Library · Liked · Queue · Settings ·           │
│          Album · Artist · Playlist   (+ Login gate when signed out)        │
└──────┬──────────────────────────┬──────────────────────┬──────────────────┘
       │ global observable state  │ playback commands    │ session tokens
┌──────▼──────────────┐  ┌────────▼─────────┐  ┌────────▼──────────────────┐
│ DATA LAYER          │  │ PLAYBACK         │  │ AUTH / SESSION            │
│ backend endpoints   │  │ engine router    │  │ in-app login page         │
│ request pipeline:   │  │ ┌──────────────┐ │  │ short-lived token capture │
│  coalesce → cache   │  │ │ official SDK │ │  │ cookie-jar persistence    │
│  → filter → send    │  │ │ open engine  │ │  │ on-demand refresh         │
│ memory + disk caches│  │ └──────────────┘ │  └───────────────────────────┘
└──────┬──────────────┘  └────────┬─────────┘
       │                   ┌──────▼──────────────────────┐
       │                   │ STREAMING (open engine)     │
       │                   │ track→source mapping        │
       │                   │ provider failover chain     │
       │                   │ stream-URL cache            │
       │                   │ local audio decoder + sink  │
       │                   └─────────────────────────────┘
┌──────▼──────────────────────────────────────────────────────────────────┐
│ CROSS-CUTTING SERVICES                                                    │
│ ad/tracker filtering engine · artwork cache · settings/profile stores     │
│ self-updater · error-to-toast bus · platform storage/audio seams          │
└───────────────────────────────────────────────────────────────────────────┘
```

Data flows downward as requests and upward as state changes; the interface
never talks to the network directly — everything passes through the data
layer, and everything the interface shows comes from the global stores.

---

## 4. Application lifecycle

### 4.1 Process startup (before the interface exists)

1. **Logging initializes** (timestamped everywhere except the browser build,
   where wall-clock time does not exist).
2. **Pending self-update is applied.** If a previous session staged a new
   binary/package, it is swapped in before any window opens, and the fresh
   process restarts into the normal path.
3. **Shared bootstrap runs**: the ad-filter engine starts (never blocking
   startup), and the persisted session is inspected (valid token present or
   not). No interface state may be touched here — the interface runtime does
   not exist yet.
4. **Window opens** (frameless, with the app's own chrome; fixed default size
   with a minimum) and the root component mounts.

The browser build differs only in that bootstrap runs concurrently with
mounting, since a browser page has no blocking startup phase.

### 4.2 Root component behavior after mount

Four one-shot initializations, each guarded so re-renders never repeat them
(important because authentication-token refreshes re-render the root
regularly — see §14):

1. Apply the persisted visual theme and seed the player volume from settings.
2. Load the local user profile (display name, avatar).
3. Run one update check if the user enabled startup checks.
4. When a session becomes available: boot the playback backend once (it must
   be created on the interface thread where the app window lives) and
   backfill the user profile once if it is still missing.

The root then renders **either** the login gate **or** the routed
application shell, selected by a single boolean in the authentication store.
No other condition controls this switch.

### 4.3 Login gate behavior

The gate shows a branded "opening login" placeholder while it kicks off the
sign-in flow exactly once (a re-entry guard prevents duplicate login pages;
failures surface an error with a retry control that resets the guard). The
moment the authentication store flips to authenticated, the gate unmounts
and the routed shell mounts. Logging out reverses the transition.

---

## 5. Authentication and session architecture

### 5.1 Core idea

The app authenticates exactly like the Spotify website: a real Spotify page
runs inside the app, the user signs in there, and the app captures the same
short-lived web-player access token the website itself uses. Consequences:

- Password, two-factor, passkeys, and social sign-in all work with zero app
  code, because the page is genuine.
- There is deliberately **no refresh token**. Long-lived login state lives in
  the page's HTTP-only session cookies, persisted in a shared browser-data
  directory. The short-lived access token (~1 hour) is mirrored to the
  operating-system credential store so startup can detect a restorable
  session.
- Token refreshes are performed *through* a live same-origin page, never by
  reusing old tokens.

### 5.2 Login flow, step by step

1. A full-screen page opens on the Spotify accounts login address (with a
   return parameter pointing at the web player). An already-signed-in user
   bounces straight through to the player automatically.
2. Injected page scripts capture the session through four layered methods, in
   reliability order:
   1. **The player's own token endpoint.** The endpoint the web player itself
      uses, whose only challenge is a locally computed one-time code (standard
      time-based one-time-password construction over a key recovered from the
      player bundle). Combined with the session cookies only this page can
      present, the app can mint a token without waiting for the page's own
      player to start.
   2. **Network-traffic observation.** A wrapper around the page's request
      function watches the player's own token responses and caches whatever
      token the real player receives — surviving future endpoint changes.
   3. **Direct legacy-endpoint polling** as a backup (often challenged by bot
      protection).
   4. **Interface fallback.** The logged-in profile widget only renders for
      signed-in users; if it appears, the flow settles shortly after,
      proceeding with a token if one was captured and empty-handed otherwise
      (the playback backend can fetch its own token from the shared cookies).
3. Only non-anonymous (fully signed-in) tokens satisfy login; guest tokens
   are ignored and polling continues.
4. The first accepted capture is delivered once to the waiting sign-in task,
   which hides the page (keeping it alive), persists the token, fetches the
   user profile, marks the session authenticated — and the interface swaps to
   the main shell.

### 5.3 The session page after login

The login page is **kept alive but hidden** as the session page, because it
is the only page whose origin permits credentialed token requests. As an
idle-resource optimization it is then **parked** on a blank page: the cookies
and storage survive, the live site stops consuming processor time, and
refreshes revive the real page on demand (navigate back, wait for load
completion, trigger the refresh routine, park again afterwards). Revival cost
is negligible because it happens roughly hourly.

### 5.4 Token refresh

- Callers that need a guaranteed-fresh token first peek at the stored expiry
  (non-subscribing read, so pages are not re-rendered by every background
  token update). If fresh, it is returned immediately.
- Otherwise a refresh is requested through the session page (preferred:
  same-origin, never blocked) with a bounded wait, falling back to the
  playback page where available.
- Concurrent refresh requests are all answered (a collection of waiters, not
  a single slot), and a lost or timed-out answer re-checks the store first —
  the periodic capture may have landed the fresh token anyway.
- Hard expiry (rejected session) clears the authenticated flag, which routes
  the interface back to the login gate; anonymous captures are treated as
  expiry with a user-visible notice.

### 5.5 Session persistence and logout

- Session cookies persist in the shared browser-data directory across
  restarts (this is what keeps the user logged in); the access token is
  mirrored to the OS credential store plus an on-disk fallback file.
- Logout clears the credential store and fallback, resets authentication
  state (interface flips to the gate), and tears down the cookie-holding
  pages so the next sign-in starts clean. Cookie clearing on logout is
  best-effort; credential clearing is authoritative.

### 5.6 Platform hosting of the login page

- **Desktop:** the page is packed into the main window next to the interface,
  which is hidden underneath; capture hides the page widget and re-shows the
  interface. Pages are only ever shown/hidden, never moved between
  containers.
- **Mobile:** the platform's view system supports a single page per window,
  so a small view-layering manager keeps the interface view at the bottom
  permanently and layers the login page above it; hiding detaches the login
  view. The login page itself is a platform browser view (not a second
  framework view), so the main interface's networking and rendering channels
  are never disturbed. Script injection, navigation, cookie acceptance,
  visibility, and teardown all go through the UI thread; read-back queries
  (load progress, page title) round-trip asynchronously.
- Page-to-native messaging uses the platform channel where one exists, and a
  document-title ferry (outbox map drained by polling, source-of-truth kept
  in page state so clobbered reads are retried, never lost) where it does
  not. Login popups and external-URL escapes are contained in-page
  (new-window requests rewritten to same-window navigation, non-web schemes
  bounced back to sign-in, navigation URLs traced in diagnostics).

---

## 6. Global state architecture

A small set of **global observable stores** is the single source of truth;
components subscribe by reading and never pass network data through
properties:

| Store | Contents |
|---|---|
| Application state | Current section, search text/results, home shelves |
| Playback state | Current track, local queue (+ shuffle-restore snapshot), playing flag, position/duration, volume, shuffle/repeat modes, playback-device id, optimistic like flag |
| Authentication state | Access token, expiry timestamp, user id, display name, avatar URL, account tier, authenticated flag |
| Filter statistics | Tracked rule count, dropped-request count, cache info, upstream failure count |
| Application error | The latest user-facing error, consumed by the toast |
| Settings | Theme, volume, engine preference, upsell hiding, update checks (persisted) |
| Search handoff | One-shot search text passed from the top bar to the Search screen |
| Interface flags | Now-playing column visibility; persisted user profile; update status/readiness |

Supporting rules that keep this cheap (see §14): background token updates
are read without subscribing wherever a subscription would restart in-flight
page fetches; one-shot boot flags prevent effect re-runs; errors clear only
when still current.

---

## 7. Data layer

### 7.1 Endpoint strategy: internal API first, public API almost never

Reads go to Spotify's **internal data API** (the same persisted-query
endpoint the website itself uses), not the public REST API — the public API
rate-limits this kind of token hard, while the internal one accepts the same
web-player token with far higher limits. Persisted-query hashes are pinned
in the client and refreshed manually when the server reports them stale.

Routed through the internal API: home feed, user playlists, liked songs,
saved albums, playlist/album/artist/track detail, and full search. Remaining
on public REST: exactly one low-volume profile read at login, and playback
*control writes* (play/pause/skip/seek/volume on explicit user action).
Request headers mimic the website (web-player platform marker plus matching
origin/referer) so the token is accepted.

Response shapes differ from public REST (nested artist profiles, wrapper
URI fields, per-operation duration fields), and all parsers tolerate missing
or variant fields rather than failing.

### 7.2 Request pipeline

Every outbound data request passes four stages:

1. **Ad-filter gate.** Blocked hosts fail immediately with a typed filter
   error and increment the drop counter. (The filter-list fetcher itself
   bypasses the gate so a list can never block its own download.)
2. **In-flight coalescing (single-flight).** Concurrent identical requests
   share one network fetch: the first caller becomes the leader, the rest
   wait on its result. Failures are never cached and followers receive a
   summarized error while the leader keeps the rich original.
3. **Memory cache.** Fixed-capacity first-in-first-out store with a short
   time-to-live for fresh hits.
4. **Disk cache with stale-while-revalidate.** Fresh-enough snapshots serve
   instantly while a background refresh updates them; expired snapshots
   block on a fresh fetch.

Cache keys are content hashes; snapshots carry their own timestamps and are
written atomically (temp file + rename) so crashes never leave half-written
data.

### 7.3 Data models

Typed models for tracks, albums, artists, playlists, paged collections,
search results, home feed, and user profile — all tolerant of partial
payloads (missing fields default instead of breaking). Derived conveniences
live with the models: comma-joined artist names, widest-artwork selection,
playability checks that treat region-blocked null tracks as skippable, and
human duration/counter formatting.

### 7.4 Search behavior

Search is debounced (quarter-second), deduplicates repeat queries, guards
against out-of-order responses with a generation counter, consumes the
top-bar handoff exactly once, and presents a top-result hero (artist over
album over track) plus per-type shelves with direct-play rows.

### 7.5 Rate-limit handling

Rate limiting is fail-fast and self-healing, never an endless spinner: at
most one short bounded wait, one retry, then a typed rate-limit error. The
home feed detects it, shows an explanatory banner, and re-fetches on a
one-minute timer until quota clears — the feed loads with no user action.

---

## 8. Ad-blocking subsystem

A compiled-rules filtering engine (Brave-style rule engine) replaces naive
list scanning:

- **Rule compilation.** AdGuard-format and hosts-format lists are split by
  syntax family before compiling (the engine accepts only one family per
  call), combined with curated Spotify ad/analytics rules and an explicit
  allow-list encoded as exception rules — so first-party Spotify domains,
  artwork hosts, the token and API endpoints, and stream hosts can never be
  blocked, even by list updates.
- **Ownership.** The engine is not thread-sharable, so it lives on a
  dedicated worker thread (inline thread-local storage on the browser
  build); checks go over a bounded message channel from any thread. An
  unparseable URL or an unavailable engine fails open (allowed).
- **Persistence and refresh.** The compiled engine is serialized to disk for
  fast restarts (falling back to compiling from text on miss/corruption);
  filter lists refresh in the background after startup without ever blocking
  it, with failures counted (not fatal) and the last good snapshot kept.
- **Enforcement points.** Every data request passes the gate before sending;
  an optional cosmetic stylesheet hides upsell/ad chrome on live Spotify
  pages (behind a user toggle, off by default).
- **Statistics.** Drops and list metadata flow through an event notification
  (not a timer) into the global stats store, so the interface re-renders only
  when something actually changed.

---

## 9. Playback architecture

### 9.1 Two engines, one router

Playback is split behind a single dispatch:

- **Official SDK engine** (Premium accounts): Spotify-owned audio via the
  Web Playback SDK running in a hidden browser page; the app sends transport
  commands and triggers playback on the SDK's device through the Connect
  control API.
- **Open engine** (free accounts, or forced by preference): resolves each
  track to a direct audio URL from independent providers and plays it
  locally through the built-in decoder and audio sink.

Selection is automatic by default (account tier decides), overridable to
either engine in Settings. Every transport control (play, pause, next,
previous, seek, volume) branches identically, and all play buttons in the
interface call one of two entry points: play-by-identifier (resolves
metadata first) or play-with-metadata-in-hand (zero network, preferred —
rows and cards always pass the track object the user already sees).

### 9.2 Official SDK path

A hidden page bootstraps the playback SDK with the session's cookies, so it
authenticates itself with zero token hand-over. It reports device readiness,
player-state changes, and errors over a message queue that is drained inside
the interface runtime (never touching interface state from the browser
thread). drives it through an injected relay object (play, pause,
track-step, seek, volume, reconnect). The page also serves as the token
refresh fallback. On the free/open path the SDK page is never created, so no
idle connection or timers exist.

### 9.3 Open path: resolution chain

Resolving a track to playable audio proceeds in order:

1. **Stream-URL cache** (memory + disk, conservative expiry well inside real
   URL lifetimes): repeat plays skip resolution entirely.
2. **Provider failover chain**, each returning an explicit outcome —
   success, cooldown (with retry delay), not-found, or error. Disabled
   providers (whose upstream services shut down) report themselves
   unavailable and are skipped without network calls. The active provider
   searches by artist + title and prefers unrestricted full-file formats
   over throttled adaptive ones.
3. **Decode and play locally** (see §9.4).

Provider health is tracked (cooldowns, uptime-list caching with static
fallbacks), and metadata/artwork always come from Spotify regardless of
which source supplies the bytes — the experience stays pure Spotify.

### 9.4 Open path: local audio pipeline

- A dedicated audio thread owns the output device (the audio objects cannot
  migrate threads). Commands (play URL, pause, resume, seek, volume,
  shutdown) arrive over a message channel.
- While idle the thread blocks indefinitely (zero wakeups); while playing it
  wakes on a short cadence to publish position.
- Full files download before decoding starts (seeks are exact, durations
  reliable); the interface clock prefers the real track duration from
  already-held metadata.
- Volume is seeded once from persisted settings — otherwise first playback
  would be silent.

### 9.5 Position synchronization without polling the interface

A single process-wide task mirrors the audio thread's published position
into playback state. It sleeps on an event notification fired only while
audio actually plays (idle: zero wakeups), takes the state lock through a
non-panicking path (skipping a tick if momentarily contended rather than
crashing), and writes only when a value actually differs — so paused or
steady state triggers no re-renders at all.

### 9.6 Artwork pipeline

Artwork loads through a hash-keyed disk cache with long time-to-live and a
bounded file count: cache hit renders instantly; miss downloads through the
ad-filter gate and stores timestamped bytes best-effort. The interface shows
a deterministic seed-colored placeholder, then a tiny blurred preview, then
the full image (blur-up pattern); empty URLs fail fast with a typed error.

### 9.7 Queue model

The app owns an explicit local play queue: track enqueueing deduplicates by
identifier (stable lists, no key collisions), next-track prefers the queue
head over device skip, shuffle snapshots the original order so un-shuffling
restores it exactly (seeded shuffle), and the queue page offers clear/remove
operations.

---

## 10. User-interface architecture

### 10.1 Shell and routing

One root component decides between the **login gate** and the **routed
shell** on a single boolean. All authenticated routes nest inside a
persistent shell layout:

```
┌──────────────┬────────────────────────┬──────────────────┐
│ top bar      │ top bar                │ top bar          │
│ side nav     │ main content (route)   │ now-playing      │
│ player bar   │ player bar             │ player bar       │
│ bottom nav   │ bottom nav             │ bottom nav       │
└──────────────┴────────────────────────┴──────────────────┘
```

- **Top bar:** navigation history, global search field (Enter hands text to
  the Search screen and routes there), user avatar/name menu (display name
  resolution, settings entry, logout).
- **Side navigation:** brand, primary links, ad-filter status badges, and a
  drag-to-resize handle (pointer capture, clamped width); collapses to an
  icon rail on medium screens and yields to a bottom tab bar on small ones.
- **Now-playing column:** large artwork, track/artist/album, position readout;
  toggleable, occupying zero width when hidden or on narrow screens.
- **Player bar:** artwork-backed transport cluster (shuffle, previous,
  play/pause, next, repeat), scrub and volume sliders, queue toggle,
  optimistic like control.
- **Toast:** user-facing errors with timed auto-dismiss (stale timers cannot
  clear newer errors) and manual dismiss.

Routes: home, search, library, liked songs, queue, settings, and
album/artist/playlist detail — each a component declaring its data needs.

### 10.2 Pages and their data

| Screen | Data | Behavior |
|---|---|---|
| Home | Playlists + liked tracks, fanned out concurrently | Time-of-day greeting; skeleton placeholders while loading; rate-limit banner with automatic retry |
| Search | Multi-type query (tracks, albums, artists) | Debounced background worker with staleness guard; top-result hero; per-type shelves; direct-play rows |
| Library | Playlists + albums + liked tracks concurrently | Client-side text filter, collection tabs, alphabetical sort; any-leg failure surfaces instead of half UI |
| Liked songs | Paginated saved-tracks feed | Skips region-blocked entries; hero with play/shuffle; incremental loading |
| Queue | Local player state only (no network) | Now-playing summary, up-next list, clear |
| Settings | Local stores + updater state (no music data) | Profile, appearance, playback-engine choice, privacy stats, updates, cache note |
| Album / Artist / Playlist | Detail + track listings | Gradient heroes, play-first/shuffle, full track tables; artist adds expandable popular tracks, discography shelf, related artists |

All pages share primitives: section headers, gradient hero headers,
paginated track tables (chunked rendering with load-more), duration labels,
and shimmer skeletons — so loading, error, and empty states look and behave
identically everywhere.

### 10.3 Theming and responsive contract

One stylesheet holds the entire design system as numbered sections, starting
with semantic design tokens (surfaces, text, accent, radii, motion). Themes
are variable sets switched by a single document attribute — instant repaint,
zero component re-renders — with the few values native code needs mirrored
as constants and guarded by drift tests (including a linter requiring every
referenced variable to be defined). Responsive breakpoints swap rail → icon
strip → bottom tabs and drop the now-playing column on narrow screens, with
loading-only animations permitted but looping decoration on ever-present
elements forbidden (they keep the renderer repainting forever).

---

## 11. Platform strategy

One shared codebase ships three renderer families through build-time
feature selection, with native desktop and mobile sharing the same
browser-view foundation and only the browser build diverging:

| Concern | Native (desktop + mobile) | Browser build |
|---|---|---|
| Interface, data, playback logic | Shared, identical | Shared, identical |
| Login page hosting | In-app page (window-packed on desktop; layered platform view on mobile) | Whole-tab redirect (the browser *is* the page holder) |
| Session token capture | Same-origin fetch inside the hosted page | Credentialed cross-origin fetch (provider-dependent) |
| Persistent key/value storage | Files under OS directories; secrets in the OS credential store | Browser local storage (base64), secrets excluded |
| Background tasks | Thread-pool tasks | Single-thread local tasks |
| Audio output | OS audio device via local decoder | Browser audio element |
| Ad-filter DNS checks | DNS-over-HTTPS resolver | Omitted (single thread) |
| HTTP stack | Cookie store, compression, timeouts | Browser fetch backend |

Narrow adapter modules isolate each seam so the rest of the app never
branches on platform: a storage adapter (opaque key/bytes operations), a
window-handle adapter (correct handle type per enabled renderer), a
task-spawning adapter (sendable vs single-thread futures), and the
mobile view-layering helpers (base-view capture, overlay attach/detach,
platform-view creation, UI-thread dispatch with result round-trips).

Hard platform facts encoded in the design: the mobile view backend hosts one
page per window; browser builds have no system clock (timestamps come from
page-provided time); mobile OS versions below the windowing-API floor are
unsupported; browser cookie and storage partitioning rules make cross-origin
session capture unreliable on static hosting (planned server-side token
proxy, not client workarounds).

---

## 12. Self-update system

- **Version identity** is injected at build time (release environment, else
  version-control tag, else package manifest) and compared numerically
  (tolerant of prefixes and suffixes).
- **Check**: fetch the latest published release metadata, match a platform
  asset by filename token, and record a human-readable status plus a
  readiness flag. Runs once at startup when enabled, or manually from
  Settings; failures report as status text, never as crashes.
- **Download**: streamed with running hash verification into a partial file
  that is atomically renamed only on success — interrupted downloads can
  never stage a half-written update.
- **Desktop staging:** unpack the single executable next to the running one
  plus a swap marker; on next launch (before any window opens) the staged
  binary replaces the current one (with a rename-then-copy dance on systems
  that forbid overwriting a running executable), cleans up, and relaunches.
  Non-tarball packages report a clean error instead of failing obscurely.
- **Android staging:** download the package into the app's private files and
  fire the system package installer through a file-provider URI (wired via a
  staged manifest entry); the OS handles consent and installation.
- The browser build omits the updater entirely.

---

## 13. Settings, profile, and personalization

- **Settings** (theme, volume with clamping and non-finite guards, playback
  engine preference, upsell hiding, update checks) persist as versioned
  documents beside the profile, load with safe defaults on any failure, and
  save asynchronously after each change with directory creation handled.
- **Profile** (display name defaulting to a friendly placeholder, avatar
  image validated by file signature and size-capped, stored encoded
  alongside) follows the same load/save pattern, publishes failures as
  toasts, and surfaces in the top-bar avatar chip (photo, else initial
  letter).
- **Theme application** is a single attribute write (see §10.3); engine and
  toggle changes snapshot-then-save so a crash cannot persist a half-written
  choice.

---

## 14. Performance architecture

This is the system's defining discipline: every potentially wasteful pattern
has an explicit, cheaper replacement. They compose — idle CPU near zero,
first paint from local data, no redundant network or renders.

### 14.1 Event-driven instead of polling

| Old pattern (forbidden) | Replacement |
|---|---|
| Position poller per track, ticking forever | One process-wide task awaiting an event notification the audio thread fires only while playing |
| Interface clock ticking always | Runs only on the engine path that needs it, slower when idle |
| Ad-stats refresh timer | Task awaiting a notification fired on drops and list refreshes, with burst coalescing |
| Audio thread wakeups while idle | Blocking receive with no work; timed receive only while playing |
| Fixed-interval page pollers after login | Timers destroyed at capture; on-demand refresh calls only |

### 14.2 Render hygiene (no wasted interface work)

- **Compare-before-write.** Global stores are only written when a value
  actually differs (cheap equality gate before acquiring the write lock),
  because merely touching a store re-renders all its subscribers.
- **Non-subscribing reads for hot values.** The authentication token is
  rewritten every couple of seconds; page data-fetchers read it without
  subscribing, or every feed would restart forever. Recovery is driven by
  explicit timers, not by token-write reactivity.
- **One-shot boot gates.** Startup effects (theme, profile, update check,
  backend boot, profile backfill) each run exactly once per process despite
  constant re-renders.
- **Debounced, generation-guarded search.** Keystrokes debounce; responses
  carry a generation so late arrivals cannot overwrite newer results.
- **No infinite decoration animation** on ever-present elements; loading-only
  animations unmount with their content.

### 14.3 Network efficiency

- **Single-flight coalescing**: N concurrent identical requests produce one
  network fetch.
- **Two-tier caching**: short-lived memory plus day-scale disk with
  stale-while-revalidate — repeat views paint instantly from snapshot while a
  background refresh keeps them fresh; failures are never cached.
- **Internal API over public API**: an order of magnitude fewer rate limits;
  batched detail queries (artist hero + discography + popular tracks in
  minimal round trips); playback uses already-in-hand metadata instead of
  re-fetching.
- **Fail-fast rate limits**: one short bounded wait, one retry, then a
  banner with automatic timed recovery — never multi-minute spinners.
- **Short-lived stream URLs** cached conservatively inside their real
  expiry; provider health tracked with cooldowns and failover order;
  instance lists cached for minutes with static fallbacks.

### 14.4 Startup path

Expensive work is ordered so the first frame never waits: credential check
before the interface exists (signals untouched), ad-filter engine warms on a
side thread with a bundled snapshot + serialized cache, persisted theme and
volume apply as pure attribute writes, profile/session backfills run once in
the background, and the playback backend boots only after authentication on
the thread that owns the window.

### 14.5 Idle resource parking

- The hidden session page is parked on a blank document when unneeded (full
  site rendering stops; cookies and storage survive; refresh revives on
  demand roughly hourly).
- Its capture scripts self-terminate on non-web documents and stop their
  timers at first capture.
- The hidden playback page is created lazily and skipped entirely on the
  engine path that never uses it.
- Update checks run once per process; package downloads stream with hashing.

### 14.6 Memory and thread discipline

- Bounded caches everywhere (fixed entry counts, file-count caps, FIFO
  eviction; snapshots carry timestamps and expire).
- Thread-affine objects (browser views, audio device) live on dedicated
  threads with message-channel command queues; cross-thread reads use
  atomics; UI-thread-only objects are never touched from workers (queued and
  drained instead).
- Async tasks use timeouts on every external wait (network clients carry
  timeouts; refresh and resolve paths are deadline-bounded) so no gate can
  hang forever.

---

## 15. Error handling and resilience

- **Typed errors** distinguish authentication failures, network failures,
  filter drops, playback failures, tier requirements, expired sessions,
  forbidden access, rate limits, backend errors, view errors, and generic
  causes — never bare strings across boundaries.
- **Layered mapping**: the data client maps rate-limit (with server-provided
  retry delay), revoked-session (with immediate logout), stale-query-hash,
  and backend-error payloads distinctly; transport writes require success
  statuses explicitly; coalesced followers receive summarized errors while
  the leading caller keeps the rich original.
- **Surfacing**: page-level failures render inline banners; everything else
  goes through a single error bus to a toast with timed auto-dismiss (stale
  timers cannot clear newer errors) and manual dismiss.
- **Session expiry** is a first-class transition: it clears the
  authenticated flag and returns the interface to the login gate, where the
  still-valid cookies usually re-authenticate silently.
- **Degradation defaults**: unknown sessions proceed to the main interface
  with background backfill rather than blocking; partial feed failures
  render available sections; missing artwork/metadata falls back to
  placeholders and defaults.

---

## 16. Testing and quality gates

- **Network-free unit tests** beside the code: durations and greeting tables,
  queue dedup/shuffle-restore invariants, parser tolerance (partial and null
  payloads), cache semantics (time-to-live, single-flight fan-in, never-cache
  errors, first-in-first-out caps, stale-while-revalidate timing),
  provider-chain states and stream-cache expiry, settings round-trips,
  avatar validation, theme-token synchronization with the stylesheet
  (including a linter requiring every referenced design token to be
  defined), and shell-grid contract assertions.
- **Performance micro-tests**: filter-engine lookup throughput targets and
  provider/cooldown ordering.
- **Gates**: static checks on all renderer targets, zero-linter-warning
  policy, full test suites on both default and headless feature sets.
  Interface behavior is verified manually by the operator running the live
  app; automation never launches the interface itself.
- **Invariants enforced in review**, not just tests: no interface runtime
  access before it exists, no state writes from browser threads (queue and
  drain), no moving realized browser views between containers, no second
  browser-data context on one directory, no timers that outlive their
  purpose.

---

## 17. Release and distribution

- Releases trigger from version tags (or a validated release script): the
  pipeline builds desktop targets (Linux system-webview glibc, macOS
  arm64+x86_64, Windows), the mobile package (explicit device-architecture
  target, generated project + manifest staging + debug-build + release
  assembly with lint workarounds + keystore signing + signature
  verification), and the browser bundle (from the builder's canonical output
  path, not the configured directory).
- Every artifact ships with a checksum plus an unversioned alias so
  "latest" links stay stable; platform asset matching is substring-based
  against both versioned and unversioned names.
- Environment prerequisites are explicit and minimal: no API credentials
  ever; desktop Linux needs system web/indicator/clipboard libraries; mobile
  needs SDK/NDK/JDK plus unversioned compiler symlinks the toolchain
  resolver requires; cross-architecture defaults follow the host unless
  overridden.
- Mobile OS floor is set by windowing-API availability; the web app deploys
  to static hosting with its path prefix baked in.

---

## 18. Known limitations and open work

- **Premium playback view on mobile** still uses the framework overlay path
  and needs the same platform-view treatment as the login page.
- **Browser login** is blocked by the provider's missing cross-origin
  headers plus unshareable session cookies on static hosting; the planned
  resolution is a small server-side token proxy, not further client
  workarounds.
- **Provider fragility**: community streaming sources and persisted-query
  hashes rotate or sunset; mitigated by failover order, cooldowns, uptime
  lists, caches, and explicit disabled flags — re-enable only behind a
  working source mapper.
- **Wishlist (designed, not scheduled):** route-hover prefetching, like/unlike
  endpoints, audio-ad substitution scaffolding (kept off), deep playback
  measurements to revisit engine defaults.
