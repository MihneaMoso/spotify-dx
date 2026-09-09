# Android Kotlin Migration — Design Document

Migrating the Spotify DX mobile interface to Kotlin while keeping the proven
native core untouched. Only the mobile UI layer is rewritten; every service
the app relies on today continues to run in the existing core and is reached
from Kotlin through a purpose-built JNI bridge.

Companion: `docs/ARCHITECTURE.md` (current system, implementation-agnostic).
That document is the behavioral spec — everything the Kotlin interface does
must be traceable back to a section there.

> **Status (2026-09): Phase 0 gate green, Phase 1 shell smoke builds, Phase 2
> login flow live-verified on-device.** The native core is a `spotify_dx`
> cdylib/rlib (`src/lib.rs`) with the versioned JNI bridge (`src/bridge.rs`);
> the owned Kotlin app in `android/` (gate + 9 screens + shell + login
> WebView + playback service + updater) is the primary Android build path
> (`scripts/build-kotlin.sh`). Session/data/ playback core calls expose
> their final §6.1 signatures as `PHASE_2_PLUS` until their phases land —
> except `beginLogin`/`refreshToken`/`currentUser`, which are real (see §8
> inversion note below). See `RULES.md` §6.9h for the load-bearing
> deviations and gotchas.
>
> Phase 2 on-device results: beginLogin → fullscreen page → cookie bounce →
> non-anonymous `/api/token` capture → mirror → gate→shell → park, all
> confirmed in logcat + screenshots; cold-start restore replays the same
> path; Settings sign-out tears down pages + store and lands back on a fresh
> anonymous login page; offline cold start surfaces Spotify's own
> challenge/notice UI (genuine page ⇒ same errors as today by construction).
> Password login and no-escape tapping remain manual (need live credentials).
>
> Phase 4 open-engine playback is live-verified (resolve → MediaPlayer, queue
> advance, seek, volume persist). Phase 5 SDK path is implemented end to end
> (core `sdkDocument`/`sdkPlay/…`/`sdkParseState` + Kotlin hidden-WebView
> driver + engine router with open fallback) but **blocked by platform**:
> Android WebView provides no EME keysystem (`EMEError: No supported
> keysystem`), so the Web Playback SDK can never reach `ready` on-device —
> every SDK attempt fails fast into the open engine (verified 2026-09).
>
> Phase 6 substantially verified on-device (2026-09): settings survive
> force-stop byte-identical (`files/settings.json`, user values intact);
> update check runs at boot against the live GitHub API ("Up to date
> (v0.1.10)"); profile/avatar/engine/privacy screens live. The
> download→stage→apply round-trip is **blocked on the release pipeline**:
> `release.yml` still publishes the legacy `dx build` scaffold APK (per-run
> keystore, wrong app) — cutover (Phase 7) must publish the owned Kotlin APK
> with a stable signing key before apply can be tested.
>
> Phase 7 CI cutover done (2026-09-09, unreleased): `android-apk` builds the
> owned Kotlin app (`build-kotlin.sh release` + bridge-compat gate),
> versionCode from `github.run_number` / versionName from the tag, signed
> with the stable release key (repo secrets) under the updater's asset name.
> Legacy mobile-renderer removal stays deferred per spec (one clean release
> with no regressions first).

---

## Table of contents

1. [Goal and non-goals](#1-goal-and-non-goals)
2. [Why this split](#2-why-this-split)
3. [Target architecture](#3-target-architecture)
4. [Boundary definition: what moves, what stays](#4-boundary-definition-what-moves-what-stays)
5. [Repository surgery first: extracting the core library](#5-repository-surgery-first-extracting-the-core-library)
6. [The JNI bridge contract](#6-the-jni-bridge-contract)
7. [Kotlin interface architecture](#7-kotlin-interface-architecture)
8. [Login flow on Kotlin](#8-login-flow-on-kotlin)
9. [Playback on Kotlin](#9-playback-on-kotlin)
10. [Data access from Kotlin](#10-data-access-from-kotlin)
11. [Settings, profile, updater, and ads on Kotlin](#11-settings-profile-updater-and-ads-on-kotlin)
12. [Threading and lifecycle rules](#12-threading-and-lifecycle-rules)
13. [Build, packaging, and CI](#13-build-packaging-and-ci)
14. [Migration phases with gates](#14-migration-phases-with-gates)
15. [Testing strategy](#15-testing-strategy)
16. [Risks and mitigations](#16-risks-and-mitigations)
17. [What explicitly does not change](#17-what-explicitly-does-not-change)

---

## 1. Goal and non-goals

**Goal.** Ship an Android application whose every pixel, navigation
transition, login page host, and playback control is Kotlin, while the
networking, authentication session logic, ad filtering, stream resolution,
audio decoding, persistence, and update machinery remain the exact,
battle-tested native code running today — called across a narrow, versioned
JNI bridge. Behavior parity with the current Android build is the acceptance
criterion for every migrated screen; no product changes ride along.

**Non-goals.**

- No rewrite of the core services (data layer, auth session logic, filter
  engine, resolver, decoder, updater checker). They move crates, not
  languages.
- No behavior or visual redesign. The Kotlin interface reproduces the
  existing information architecture, theming tokens, responsive contract,
  and copy; redesigns happen after parity, never during.
- No changes to desktop, web, or iOS targets. The core crate stays shared;
  only the Android application shell is replaced.
- No new backend endpoints, no new Spotify integrations, no new permissions
  beyond what background playback already requires (see §9.6).

---

## 2. Why this split

Three facts from the current system force this exact boundary:

1. **The interface is where the Android pain lives.** The shared
   cross-platform interface renderer hosts exactly one page per window on
   Android, which forced an entire view-layering subsystem (base-view
   capture, overlay attach/detach, visibility workarounds) plus a
   platform-view workaround for the login page — all to emulate what the
   Android view system does natively. A Kotlin interface deletes that whole
   category instead of extending it.
2. **The services are where the value lives.** Session capture (four layered
   methods plus token-refresh fan-out), the internal-API data layer with
   coalescing and two-tier caches, the Brave-style filter engine, the
   provider failover chain, and the idle-CPU discipline represent the
   overwhelming majority of debugging already paid for. Rewriting any of
   them risks re-learning every outage documented in the project history.
3. **The precedent already exists in-repo.** Self-update installation already
   crosses the boundary the other way today: native code drives a Kotlin
   installer object and a file provider through JNI, staged into the
   generated project by script. The migration generalizes a proven pattern,
   and the file-provider/installer pieces transfer verbatim.

---

## 3. Target architecture

```
┌────────────────────── Android application (Kotlin) ──────────────────────┐
│  Interface layer (new)                                                    │
│  ┌────────────┐  ┌──────────────────┐  ┌───────────────────────────────┐  │
│  │ Navigation │  │ Screens (9 + gate)│  │ Shell: top bar, nav rail/tabs │  │
│  │ graph      │  │ Home Search …     │  │ player bar, now-playing, toast│  │
│  └────────────┘  └──────────────────┘  └───────────────────────────────┘  │
│  ┌──────────────────┐  ┌──────────────────────────────────────────────┐  │
│  │ ViewModels       │  │ Platform services (Kotlin-owned)             │  │
│  │ (one per screen, │  │ login/session WebViews · media playback      │  │
│  │  reactive state) │  │ foreground service + media session · installer│  │
│  └──────────────────┘  └──────────────────────────────────────────────┘  │
├────────────────────── JNI bridge (new, versioned, narrow) ───────────────┤
│  C-ABI exports (Kotlin→core)  ·  listener callbacks (core→Kotlin)         │
├────────────────────── Native core (existing, untouched) ─────────────────┤
│  auth session/token logic · data fetchers · filter engine · resolver +    │
│  providers · audio decoder · caches · settings/profile stores · token     │
│  store · update checker/downloader/stager                                 │
└───────────────────────────────────────────────────────────────────────────┘
         Spotify backends (unchanged): web player, internal data API,
         Connect control API, provider CDNs, filter-list upstreams
```

Direction of dependence is strict: Kotlin depends on the bridge; the bridge
depends on the core; the core never depends on Kotlin (except invoking
registered listener callbacks — see §6.3). The core must build and pass its
existing test suites with the interface code absent.

---

## 4. Boundary definition: what moves, what stays

### 4.1 Moves to Kotlin (interface + Android-owned platform behavior)

| Current responsibility | Kotlin owner | Notes |
|---|---|---|
| Root login-gate switch | Gate screen + session observer | Same boolean semantics (§6 ARCHITECTURE) |
| All 9 routes + login placeholder | Navigation graph + 9 screens + gate screen | §7 |
| Shell (top bar, rail/tabs, player bar, now-playing, toast) | Shell composables + player ViewModel | §7 |
| Theming + responsive contract | Theme system + adaptive layouts | Tokens ported 1:1 (§7.4) |
| Login-page hosting (overlay view) | Activity/Fragment-hosted platform view | Keeps today's proven pattern (§8) |
| Hidden playback page (SDK path) | Hidden platform view, Kotlin-driven | Same SDK document, new driver (§9.5) |
| Audio output device | Media playback stack (§9.6) | Decoder may stay native; output moves |
| Background playback, lock-screen controls | Foreground service + media session | New capability, no current equivalent |
| Package install trigger | Existing installer object + provider, transferred verbatim | Already Kotlin (§11.4) |
| App manifest, Gradle project, launcher activity | Owned project (no generator) | §13 |

### 4.2 Stays native (untouched, called over JNI)

| Service | Why it stays | Bridge surface it needs |
|---|---|---|
| Session/token logic (capture orchestration, refresh fan-out, expiry transitions) | Outage-hardened; tied to page-script protocols | Start login, deliver session events, refresh token, logout |
| Data fetchers (playlists, library, search, album/artist/detail, profile) | Rate-limit behavior + parsers debugged against live responses | One call per screen data need, paginated where paged today |
| Filter engine + list supply + stats | Dedicated-thread ownership model; serialized cache | URL check, drop counter snapshot, refresh trigger |
| Stream resolver + provider chain + URL cache | Failover/cooldown semantics; sole active provider contract | Resolve track → stream URL; availability introspection |
| Audio decoder | Verified format/seek behavior | Decode bytes → PCM frames (or file/URL handoff — §9.6 decision) |
| Settings/profile/token stores | Persistence formats + fallback chains | CRUD + change notifications |
| Update check/download/stage | Hash verification, asset matching, staging layout | Check now, fetch update, apply/stage status |
| Artwork cache | Hash-keyed disk cache with TTL | Fetch bytes by URL (or keep Kotlin-side loader reading the same files) |

### 4.3 Deliberately undecided here (decided in §9.6)

Exactly one fork needs a decision before Phase 4: where decoded audio meets
the speaker. The resolver output (a URL) is bridge-trivial; the choice is
whether PCM frames cross JNI (keep the native decoder + sink timing) or the
Kotlin side streams the URL itself (native code only resolves). Default
recommendation is in §9.6 with rationale.

---

## 5. Repository surgery first: extracting the core library

This is Phase 0 and it blocks everything else. Today the crate is
binary-only (empty library entry point, no library target): interface code,
platform entry points, and services share one compilation unit.

1. **Introduce a library target** alongside the binary. Move every
   service module into it unchanged: authentication/session logic, data
   fetchers and models, filter engine, streaming resolver/providers/cache,
   audio decoding, artwork cache, settings/profile/token stores, updater
   checker/stager, shared state types and error types.
2. **Keep behind** (binary-only): the declarative interface tree, the
   router, theme application, the desktop/mobile interface entry points and
   window setup, and the interface-renderer webview hosts (desktop overlay
   manager, mobile view-layering, session-view drivers). The desktop and
   mobile builds must compile and behave identically after the move — the
   binary becomes a thin shell over the library.
3. **Add a C-ABI surface module** in the library (`bridge`): the only new
   native code in this phase, exposing plain functions over stable C types
   (see §6). No interface logic moves yet.
4. **Gate:** desktop + mobile + headless static checks, linter clean, full
   test suites green, and a smoke rule — no behavior change on any existing
   target. Until this gate passes, no Kotlin code is written.

---

## 6. The JNI bridge contract

The bridge is the project's most load-bearing new artifact. Design rules:

- **Narrow and versioned.** Every crossing is an explicit function with a
  schema version negotiated at startup (`bridge_version() -> int`; Kotlin
  refuses to proceed on mismatch with a clear diagnostic, never undefined
  behavior). Additive changes only within a major version.
- **Boring types only.** Strings (UTF-8), 64-bit integers, booleans, and
  byte arrays cross the boundary. Structured data crosses as JSON text
  using the core's existing tolerant models — no shared-memory layouts, no
  object graphs, no exceptions across the boundary (all failures return
  explicit error codes/messages the Kotlin side maps to its own error
  hierarchy, mirroring today's typed errors).
- **No blocking calls on the interface thread.** Every bridge call is either
  synchronous-and-cheap (cache hits, state reads, settings access) or
  explicitly asynchronous (network fetch, resolve, refresh), completing via
  listener callback or a polled future. Long work never holds the UI thread.
- **Core never allocates interface objects.** All view/widget/activity
  references stay Kotlin-side; the core only ever receives opaque handles it
  was given (symmetric to how the installer object works today).

### 6.1 Kotlin → core (representative surface; exact shapes are set in Phase 1)

- Session: `session_status() -> json`, `begin_login()`, `logout()`,
  `refresh_token_async(callback)`, `current_user() -> json|null`.
- Data: `get_home()`, `search(query, types, limit)`,
  `get_playlist(id)`, `get_album(id)`, `get_artist_page(id)`,
  `get_liked_tracks(limit, offset)`, `get_library(kind, limit, offset)` —
  each returning JSON payloads or paged envelopes matching today's models,
  including the same partial-failure tolerance (a failing leg yields
  defaults, never a crash).
- Playback control: `play_track(track_json)`, `play_uri(uri)`,
  `pause()`, `resume()`, `next()`, `prev()`, `seek(ms)`, `set_volume(0..1)`,
  `enqueue(track_json)`, `clear_queue()`, `set_shuffle(bool)`,
  `set_repeat(mode)`, `resolve_stream(track_json) -> url|null`.
- Filter: `should_block_url(url) -> bool`, `filter_stats() -> json`,
  `refresh_filters()`.
- Settings/profile: `get_settings/set_settings(json)`,
  `get_profile/set_profile(json)`, `set_avatar(bytes, mime)`.
- Updater: `check_for_updates()`, `update_status()`,
  `download_update()`, `apply_update()` (hands the staged package to the
  existing installer flow).

### 6.2 Paged data contract

Today's paginated feeds (liked songs; library windows) stay paginated
across the bridge with identical semantics: fixed page sizes, total counts,
`null`-track skipping for region-blocked items, and append-only accumulation
owned by the Kotlin ViewModel. No unbounded lists cross JNI.

### 6.3 Core → Kotlin (events)

The core pushes, never polls, over registered listener interfaces held as
global references:

- **Session events**: authenticated / expired / token refreshed (payload:
  minimal user object). Replaces polling the session store.
- **Playback events**: track changed, playing/paused, position ticks (at the
  same event-driven cadence the core already uses — no timer on the Kotlin
  side), queue changed, device ready/lost, errors.
- **Update events**: check finished (update available / up to date / failed
  with message), download progress, staged-ready.
- **Filter events**: drop bursts for the privacy panel.

Listeners are registered/unregistered with the interface lifecycle, invoked
on background threads, and marshal to the main thread on the Kotlin side.
Listener teardown on destroy is mandatory (no dangling global references).

### 6.4 Data marshaling choices

- JSON text for all structured payloads (tracks, albums, search results,
  settings, profile). Slightly more bytes than binary formats; vastly easier
  to version, log, and debug — and payloads are small (pages, not streams).
- Raw bytes only for avatar uploads (Kotlin → core) and artwork blobs if
  the Kotlin loader reads through the core cache.
- Artwork strategy (decide in Phase 3, default recommended): Kotlin image
  loader reads the core's disk cache files directly by content-hash path
  convention (zero JNI traffic for pixels, instant cache hits), falling back
  to network fetch through the core gate on miss. The placeholder →
  blur-preview → full progressive pattern is reproduced with platform
  primitives.

---

## 7. Kotlin interface architecture

One navigation graph, one screen per current route, one ViewModel per
screen. No business logic in composables: screens render state and forward
intents; ViewModels own pagination, retry, and refresh; the core owns data.

| Current screen | Kotlin screen | Data source (bridge) | Notes |
|---|---|---|---|
| Login gate | Gate screen + `LoginViewModel` | Session events | Placeholder → web view → shell swap; retry control preserved |
| Home | `HomeScreen` + VM | `get_home()` | Greeting, shelves, skeleton placeholders, rate-limit banner + auto-retry |
| Search | `SearchScreen` + VM | `search()` | 250ms debounce, staleness guard, top-result hero, per-type shelves |
| Library | `LibraryScreen` + VM | Library trio concurrently | Tabs, text filter, A–Z sort; any-leg failure surfaces |
| Liked | `LikedScreen` + VM | Paged liked-tracks | Hero + play/shuffle, incremental loading, blocked-track skipping |
| Queue | `QueueScreen` + VM | Local player state (no fetch) | Now-playing, up-next, clear |
| Settings | `SettingsScreen` + VM | Settings/profile/updater/filter surfaces | Profile, appearance, engine radios, privacy stats, updates, cache note |
| Album / Artist / Playlist | Detail screens + VMs | Detail fetchers | Heroes, tables, expanders, discography/related shelves |

Shell responsibilities map 1:1: top bar (history, search handoff, avatar
menu, logout), navigation rail/tabs with the same responsive breakpoints,
now-playing column with the same width/visibility rules, player bar
(transport cluster, scrub/volume sliders, queue toggle, optimistic like),
toast (timed auto-dismiss with stale-timer guard).

### 7.1 State mapping

Each global store becomes an observable holder in the owning ViewModel (or
an application-scoped holder for session/player/filter-stats), fed by the
initial bridge read plus the §6.3 event streams. Two subtleties carry over:

- The authentication token refreshes frequently in the background; screens
  must not re-fetch on token updates (recovery stays timer-driven, exactly
  as today).
- One-shot boot work (theme apply, volume seed, profile load, update check,
  backend boot, profile backfill) keeps its run-once guards — re-subscription
  storms are the same hazard in reactive UI frameworks.

### 7.2 Navigation

Typed destinations mirror the nine routes plus detail arguments (album,
artist, playlist identifiers). The top-bar search handoff stays one-shot
(consumed on arrival, never re-seeding on back navigation). Deep-link
handling for the app's own addresses is explicitly out of scope for the
migration (no new intent filters beyond what packaging already needs).

### 7.3 Error surfaces

Inline banners for page-level failures (with the rate-limit message +
automatic retry preserved), toast bus for the rest, optimistic like with
silent fallback — same rules, platform components.

### 7.4 Theming and responsive contract

Port the design-token set 1:1 (surfaces, text, accent, radii, motion) into
the platform theme system; theme switch remains an instant repaint with no
data reload. Keep the breakpoint contract (icon rail, bottom tabs,
now-playing width/visibility) and the animation rule (loading-only motion;
no looping decoration on persistent elements).

---

## 8. Login flow on Kotlin

The current in-app login behavior is the spec; only the host changes:

1. Gate screen shows the placeholder and asks the core to begin login.
2. The **Activity hosts a platform browser view fullscreen** over the gate
   (same proven pattern as today: layered above the interface, which is
   never touched, reparented, or rebuilt).
3. Session capture keeps its layered methods in priority order: the
   player's token endpoint with locally computed one-time code → page-traffic
   observation → legacy polling → logged-in-DOM fallback, anonymous tokens
   ignored, first-wins delivery.
4. On capture: persist token, fetch profile, mark authenticated → gate swaps
   to the shell. The page stays alive hidden as the refresh channel and is
   parked on a blank document when idle (cookies/storage survive; refresh
   revives on demand).
5. Popup/new-window containment and external-scheme bounce-back stay exactly
   as specified today (same-window rewrites, fail-closed popups, scheme
   guard with sign-in recovery, navigation-URL tracing in diagnostics).
6. Logout clears credentials, resets session state, tears down cookie pages,
   and wipes cookies best-effort.

What moves where: page hosting, visibility, cookie acceptance, progress and
title polling, script injection, and navigation tracing become
Activity/Fragment code; capture orchestration, token validation, persistence,
refresh fan-out, and expiry transitions stay in the core behind §6.1 calls
and §6.3 events. The `document.title` ferry exists only because the current
host cannot define Java bridge classes — a Kotlin host defines a proper
script interface instead (strictly more reliable; same message vocabulary).

---

## 9. Playback on Kotlin

### 9.1 Engine router (unchanged semantics)

Automatic-by-default (account tier decides), overridable to either engine in
Settings. Play-by-identifier resolves metadata first; play-with-metadata
(rows/cards pass the visible track object) never re-fetches. Queue-first
next-track, dedup-by-identifier ingestion, snapshot-restore unshuffle, and
the no-network queue screen all carry over verbatim.

### 9.2 Official SDK path

Same document (vendor SDK bootstrap, token callback, relay object,
state/error listeners), hosted in a hidden platform view and driven from
Kotlin instead of the core: play/pause/track-step/seek/volume map to relay
calls, device readiness and player-state events map to the playback event
stream, transport commands map to the Connect control calls the core already
makes. Lazy creation (never built on the engine path that doesn't use it)
carries over.

### 9.3 Open path: resolution (stays native)

Unchanged behind `resolve_stream(track)`: stream-URL cache first, ordered
provider failover with explicit success/cooldown/not-found/error outcomes,
unavailable providers skipped without network calls, unrestricted full-file
formats preferred, metadata/artwork always from the music service. Kotlin
never implements provider logic.

### 9.4 Open path: decoding (stays native)

Verified format/seek behavior (multi-codec decode, accurate seek with
decoder reset, gapless prebuffer planning) remains the reference. What
crosses the boundary depends on §9.6.

### 9.5 Position sync (stays event-driven)

The core keeps publishing position only while audio plays; Kotlin mirrors it
into player state with compare-before-write. No polling ticker is introduced
on either side.

### 9.6 The one decision: where decoded audio meets the speaker

**Recommended: Kotlin streams the resolved URL with the platform media
stack.** The core resolves and caches the stream URL; Kotlin hands it to the
platform player (background-capable), which owns output, audio focus,
notifications, and lock-screen controls. Rationale: notifications and
lock-screen transport are required product surface with no current
equivalent; the platform stack provides them (plus gapless/seek/volume
semantics) instead of reimplementing JNI audio plumbing; the resolver cache
keeps repeat plays instant; the native decoder remains available as fallback
for formats the platform stack mishandles.

**Explicitly rejected:** piping PCM frames over JNI into platform audio
sinks (high sustained bridge traffic, duplicated clock/sync logic, two
volume truths), and keeping the current portable audio output on Android
(works, but yields no notifications, no media-session integration, and
weaker background guarantees).

### 9.7 Background playback and media session (new capability)

Requires a foreground service (persistent notification, audio-focus
handling, interruption reconciliation into player state) and a media-session
integration (lock-screen transport, headset/Bluetooth controls, system
volume mapping). Neither exists today; both are Kotlin-owned, driven by the
§6.3 playback event stream. No interface rebuild on audio-focus changes —
state reconciliation only.

---

## 10. Data access from Kotlin

- One bridge call per screen data need (§6.1), same pagination, same
  partial-failure tolerance (failing legs default, never crash).
- Rate-limit behavior is contractual: bounded wait, single retry, typed
  error → banner + timed auto-retry on Home. Kotlin must not add its own
  retry loops around bridge calls.
- Search keeps debounce, dedup, staleness guard, and one-shot handoff
  consumption — implemented in the ViewModel, same constants.
- Detail screens reuse in-hand objects for playback launches (no metadata
  re-fetch on play).
- The web profile read stays the single low-volume exception alongside
  Connect *control writes* (user-initiated transport only).

---

## 11. Settings, profile, updater, and ads on Kotlin

- **Settings/profile:** same documents, same defaults-on-failure, same
  validation (volume clamping, avatar signature + size cap), same
  snapshot-then-save discipline; avatar bytes cross JNI once.
- **Updater:** the checker/downloader/stager stay native (hash verification,
  asset matching, atomic staging); the existing Kotlin installer object +
  file provider transfer verbatim; check-once-per-process and manual
  check/apply wiring mirror today's gates; the update prompt surfaces only
  on staged-ready.
- **Ad filtering:** URL checks and stats stay behind the bridge (dedicated
  worker thread untouched); the Settings privacy panel renders the same
  counters; list refresh stays background-only, never blocking startup; the
  cosmetic layer and the upsell toggle keep their current default-off
  posture.

---

## 12. Threading and lifecycle rules

These are the non-negotiable invariants; most current Android bugs trace to
their violation:

1. **Never block the interface thread.** Bridge calls are cheap-sync or
   async-with-callback; network, decode, and resolve run on core workers.
2. **Core callbacks arrive off-thread.** Every §6.3 listener marshals to the
   main thread on the Kotlin side before touching interface state.
3. **No view reparenting, ever.** Pages are shown/hidden/detached in place;
   realized views are never moved between containers.
4. **No second cookie jar.** One shared cookie store for login and playback
   pages; logout tears pages down so the next login starts clean.
5. **Core outlives the interface.** The native runtime is owned by the
   Application object, not the Activity: rotation, multi-window, and process
   recreation must never restart downloads, decoding, or the session. View
   state (scroll, text input, navigation back-stack) is the only thing
   allowed to reset, per platform norms.
6. **Listener and reference hygiene.** Every registered callback is
   unregistered on destroy; every global reference is released with its
   owner; fire-and-forget native calls never leave exceptions pending.
7. **Fail-open filtering, fail-closed navigation.** Unknown URLs are allowed;
   unknown page navigations stay in-view; unknown bridge versions refuse
   loudly with diagnostics, never undefined behavior.

---

## 13. Build, packaging, and CI

- **Owned Gradle project** (no generator): application ID aligned (today's
  template default leaks a placeholder ID — fix it in the migration),
  `minSdk` floor kept for windowing-API availability, release signing,
  lint-vital workarounds as needed, and the existing updater-provider
  staging folded in as a first-class source set instead of a post-step.
- **Native library build**: the core compiles to a per-ABI shared library
  (64-bit mobile first, matching today's shipped architecture; further ABIs
  only with measured demand), linked into the APK's library directory with
  debug symbols retained per the existing keep-rules; the Java/Kotlin
  `loadLibrary` call matches the shipped soname exactly.
- **Bridge compatibility check** runs in CI: the Kotlin-declared native
  signatures are verified against the core's exported symbols on every
  build, so a renamed native function fails the pipeline instead of the
  app (the mobile equivalent of today's zero-warning policy).
- **CI matrix**: existing desktop/web/headless checks untouched; Android job
  becomes build-native-library → assemble → sign → verify, plus interface
  unit tests and the core's network-free suites. Release assets keep the
  checksum + alias conventions; the APK remains the signed universal
  artifact.
- **Local loop**: the existing device build/install script pattern carries
  over (uninstall-before-install to dodge signature conflicts, explicit
  device architecture, launch main activity), extended with logcat filters
  for the bridge tag and a screen-capture step for visual verification.

---

## 14. Migration phases with gates

Each phase ends green and shippable; no phase starts until the previous
gate passes. Behavior parity is verified against the current Android build
side by side.

- **Phase 0 — Core extraction (§5).** Library target, C-ABI surface module,
  desktop/mobile/headless unchanged. *Gate:* all existing static checks,
  zero warnings, full test suites; desktop behavior identical.
- **Phase 1 — Bridge + shell smoke.** Owned Gradle project, native library
  linkage, bridge version handshake, gate screen + one static screen
  (Settings, no fetch) rendering over JNI-provided settings. *Gate:*
  signature-compatibility CI check green; screen matches current build
  pixel-stable-ish; no crashes on rotation.
- **Phase 2 — Login flow (§8).** Platform login view, capture drivers,
  session events, gate↔shell switch, logout teardown. *Gate:* full
  sign-in/sign-out/restore cycle on device; airplane-mode and wrong-password
  paths show the same errors as today; no external app or browser escapes.
- **Phase 3 — Browse screens (§7 table, §10).** Home, Search, Library,
  Liked, Queue + shell + toast + theming. *Gate:* each screen against the
  current build (loading/empty/error/loaded states, pagination, retry,
  banners); filter stats visible; artwork progressive loading intact.
- **Phase 4 — Open-engine playback (§9.3–9.7).** Resolver bridge, platform
  media stack, position sync, queue/shuffle semantics, foreground service +
  media session. *Gate:* gapless-feel advance, exact seek, volume
  persistence, interruption handling, background + locked-screen playback.
- **Phase 5 — SDK path (§9.2).** Hidden platform view, relay driving,
  device readiness, transport mapping. *Gate:* Premium account end-to-end
  with device handoff behavior matching today.
- **Phase 6 — Updater/settings/profile parity (§11).** Installer flow,
  avatar upload, engine radios, privacy panel. *Gate:* staged-update
  install round-trip on device; settings survive restart.
- **Phase 7 — Cutover.** Android build leaves the generator path; CI
  publishes the Kotlin APK as the release artifact; legacy mobile renderer
  code is removed only after one full release with no regressions.
- **Phase 8 — Hardening.** Rotation/process-death torture, low-memory
  behavior, offline behavior per screen, accessibility pass, performance
  audit against §14 of the architecture doc (idle CPU, first paint,
  redundant-request counts).

---

## 15. Testing strategy

- **Core:** unchanged network-free suites (unchanged code paths must keep
  passing untouched — the strongest migration invariant).
- **Bridge:** signature-compatibility CI check; round-trip tests for every
  call with canned payloads (success, malformed, error); listener
  registration/teardown leak checks; version-mismatch refusal test.
- **Interface:** ViewModel unit tests (pagination append, debounce/staleness,
  retry/backoff, error mapping, one-shot handoff consumption); screenshot
  comparison for shell + each screen state (loading/content/empty/error);
  navigation tests (deep routes, back-stack, handoff consumption).
- **On-device:** login/logout/restore matrix, airplane-mode behavior,
  rotation + process-death recovery, background playback + interruptions
  (call, notification, headset), update install round-trip, cold-start
  timing vs today.
- **Parity checklist per screen:** loading → content → empty → error →
  retry, identical copy, identical navigation targets, identical playback
  outcomes.

---

## 16. Risks and mitigations

| Risk | Likelihood / impact | Mitigation |
|---|---|---|
| JNI threading/lifecycle crashes (the historic Android failure class) | High / fatal | §12 rules as review checklist; bridge-compat CI; rotation/death torture in Phase 8; keep all JNI surface in one reviewed module |
| Playback behavior drift (timing, gapless feel, volume) | Medium / high | Parity harness: scripted playlist run scored on time-to-first-audio, seek accuracy, advance smoothness vs current build |
| Login-flow regressions (provider page changes) | Medium / blocking | Capture drivers unchanged in core; containment + telemetry unchanged in twin Kotlin host; manual login matrix each release |
| Provider/key rotation (stream sources, query hashes) | Ongoing / medium | Unchanged core handling (failover, cooldowns, hash refresh); Kotlin needs no changes by construction |
| Scope creep into redesign | Medium / schedule | Parity-first rule: any visual/behavior delta vs current build fails its phase gate |
| ABI/device fragmentation | Low / medium | Ship 64-bit first; expand only with demand; keep minSdk floor |
| ProGuard/R8 stripping bridge symbols | Medium / fatal | Explicit keep rules for the C-ABI surface + installer classes; verified by the compat check |

---

## 17. What explicitly does not change

To prevent accidental renegotiation mid-migration: the web-session auth
model (no credentials in app code, no refresh token, cookie-jar sessions);
the internal-API-first data strategy and its caching/coalescing semantics;
the filter engine, rule sources, and default-off cosmetic posture; the
dual-engine playback model and its selection table; the no-paywall product
promise; the self-update check/download/stage semantics; the settings and
profile document formats (the Kotlin app must read files the current app
wrote — forward/backward compatible by test); the release checksum/alias
conventions; and the zero-warning, tests-green, never-touch-the-running-app
development discipline.
