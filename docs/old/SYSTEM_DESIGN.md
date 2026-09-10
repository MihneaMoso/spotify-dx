# System Design — Spotify DX rework

Status: proposal (not yet implemented). Companion docs: `docs/RESEARCH.md`,
`docs/ROADMAP.md`.
Constraints inherited from `AGENTS.md`/`RULES.md`: never touch `dx serve`; verify with
`cargo check --features desktop` + clippy + tests; never run the app.

---

## 1. Goals

1. **UI parity with open.spotify.com**: same information architecture — left library
   rail, main feed with shelf rows/cards, right "Now playing" view, top bar, bottom
   player bar; pages for Home, Search, Library, Playlist, Album, Artist, Liked Songs,
   Queue, Settings.
2. **Theme**: dark/deep-blue default, modern and fast; themes are code artifacts (CSS
   variable sets), switchable **only** in Settings, persisted across restarts.
3. **Backend**: keep the working web-session auth; make the data layer feel instant via
   coalescing, stale-while-revalidate caching, prefetching; replace the toy ad-blocker
   with a Brave-style compiled-rule engine.
4. **Responsiveness first**: no spinner where a snapshot exists, optimistic playback UI,
   virtualized long lists.
5. **Multi-source playback ("no paywall")**: every signed-in user gets full-track
   playback regardless of account tier. Two engines are built side by side — the
   Spotify Web Playback SDK (official, Premium) and an open multi-source engine (FLAC
   via TIDAL/Qobuz/Amazon community backends, YouTube audio fallback) — and the
   testing phase decides empirically which one ships as the default long term.

## 2. Non-goals

- No wrapping open.spotify.com in a visible WebView (SpotiCap route) — we keep native
  Dioxus rendering.
- No shipped webpack/protobuf ad-service tampering (fragile; see §7).
- No ideological commitment to either playback engine up front: the SDK-vs-open-source
  question is answered with measurements (reliability, latency, quality), not opinion.

> **Owner decision (2026-08):** multi-source FLAC/YouTube streaming IS in scope, and
> free accounts get full-track playback — a no-paywall philosophy. See §6.3/§6.7 for
> the mechanism and §7 for the accepted trade-offs.

## 3. Architecture overview

```
┌────────────────────────── Desktop window ──────────────────────────┐
│  Dioxus UI (native, themed)                                        │
│   AppLayout: TopBar │ LibraryRail │ MainView ⇄ NowPlayingView      │
│              PlayerBar (persistent)                                │
│   Routes: Home Search Library Playlist Album Artist Liked Queue    │
│           Settings                                                 │
└──────────┬──────────────────────────────┬──────────────────────────┘
           │ global signals (state.rs)    │ commands
┌──────────▼──────────────────────────┐   ┌▼─────────────────────────┐
│ DATA LAYER  src/spotify/            │   │ PLAYBACK src/player/     │
│  pipeline: dedup→cache→filter→send  │   │  engines: sdk | open     │
│  store/: memory LRU + disk SWR      │   │  optimistic PLAYER_STATE │
│  prefetcher: hover/route warm-up    │   │  local queue management  │
│  images/: disk-cached artwork       │   └────────────┬────────────┘
└──────────┬──────────────────────────┘                │
                                           ┌──────────▼─────────────────────┐
                                           │ STREAMING src/streaming/       │
                                           │  odesli/isrc map → resolve URL │
                                           │  tidal/qobuz/amazon/youtube    │
                                           │  url cache · local audio sink  │
                                           └────────────────────────────────┘
        AUTH (unchanged): webview_login + token_store + keychain
```

Key decision: **keep the current skeleton** (auth, filtered-client pattern, global
signals are sound and debugged); rebuild the *presentation*, the *caching/prefetch
tier*, and the *filter engine* around it.

Playback is deliberately dual-track: the hidden-WebView SDK engine (works today,
Premium-only) and a new open engine (`src/streaming/`) that resolves playable URLs from
third-party sources and plays them locally — so any account tier gets full-track
playback. Which engine becomes the default is decided in the testing phase (§8).

---

## 4. UI design

### 4.1 App shell (matches open.spotify.com structure)

CSS grid, 3 columns × 2 rows:

```
topbar    topbar         topbar
rail      main           now-playing
player    player         player
```

- `--rail-width`: 280 px, drag-resizable (existing resizer pattern preserved), collapses
  to an icon rail < 1000 px, bottom-nav < 820 px (keep existing responsive contract).
- **TopBar**: back/forward history buttons (router-backed), centered search field
  (typing routes to `/search?q=`), user avatar chip menu (profile, settings, logout).
- **NowPlayingView** (right column, ≥1280 px only): large current-track art,
  artist/album links, queue tab. Toggleable; hidden state frees the column to `main`.
- **PlayerBar**: unchanged 3-zone layout (track info / transport+progress /
  volume+queue), restyled to tokens; add like button and queue-open affordances.

### 4.2 Page specs (parity checklist)

| Page | Must have |
| --- | --- |
| Home | Greeting header; 8-tile "jump back in" shortcut grid; shelf rows: recently played, featured, new releases, recommendations; horizontal scroll-snap carousels |
| Search | Debounced (250 ms) query; "Top result" hero card + Songs list + per-type shelves; browse-category grid when query empty |
| Library | Tabs Playlists/Artists/Albums/Liked; sort (recency/alphabetical) + text filter; grid/list density toggle |
| Playlist | Gradient hero (cover, owner, counts, big play/shuffle), sticky action bar on scroll, track table (#, title+artists, album, date-added, duration, like), double-click plays context, context menu (add to queue/playlist) |
| Album | Same minus date-added column; release year + label footer |
| Artist | Hero banner, listener chip, Popular tracks (top 5 expandable), Discography carousel, "Fans also like" shelf |
| Liked Songs | Virtualized infinite scroll over paginated `/me/tracks`, gradient hero in deep-blue accent |
| Queue | Now playing + up-next, drag reorder (local queue), remove, clear |
| Settings | Appearance (theme picker — §5), Playback, Privacy (ad-block stats/toggles), Cache. No other page exposes theme controls |

### 4.3 Responsiveness rules

- Shelves use CSS grid auto-flow columns sized by `clamp()`; no JS measuring except the
  existing sidebar resizer.
- Track tables render rows lazily (windowed rendering: render ± viewport with spacer
  divs); target 60 fps scroll on a 10 k-row Liked Songs list.
- Artwork always renders from local cache first (§6.4); placeholder uses seed-color
  gradients (generalize today's `color_from_seed`).

---

## 5. Theme system

- `assets/main.css` keeps its numbered sections. Section 1 becomes **tokens only**:
  ```css
  :root { /* semantic tokens: --bg-surface0..3, --text-*, --accent, --motion-* ... */ }
  [data-theme="deep-blue"] { /* default: current palette retuned */ }
  [data-theme="onyx"]      { /* near-black alternative */ }
  ```
  Every component rule consumes variables only — a theme swap touches zero selectors.
- Selection lives in a settings store: `{ theme, volume, adblock toggles, … }` persisted
  at `{cache_dir}/settings.json` (`dirs` crate already present).
- Applied at bootstrap by setting `data-theme` on `<html>` via
  `dioxus::document::eval`; Settings changes re-eval only that attribute — instant CSS
  repaint, no re-render storm.
- `src/ui/theme.rs` keeps mirroring the few constants Rust needs (placeholder colors,
  layout metrics) with a unit test asserting sync with the CSS.

---

## 6. Backend design

### 6.1 Request pipeline (replaces bare `api_get_json`)

```
caller → coalesce(url) ── inflight? join existing future (one fetch, N waiters)
             │ else
             ▼
        memory cache (TTL 5 min, LRU size cap)
             │ miss/stale
             ▼
        disk snapshot (stale-while-revalidate window ~24 h)
             │ hit-stale → return immediately, refresh in background
             ▼
        adblock gate → reqwest → classify (401/403/429 logic unchanged)
             │ success
             ▼
        write both caches → resolve all joined waiters
```

New module `src/spotify/store.rs`: `parking_lot` maps + in-flight future sharing keyed by
URL; keeps today's classify/retry semantics intact underneath.

### 6.2 Prefetch & batching

- `get_home()` fans out concurrently (`futures::join_all`) instead of the current
  sequential featured → new-releases → recommendations chain.
- Route-hover prefetch: hovering a card >150 ms warms the store for the target route
  (fire-and-forget; coalescing makes cancellation unnecessary).
- Artwork URLs are handed to the image pipeline (§6.4) at fetch time.

### 6.3 Playback — two engines behind one facade

A `PlaybackEngine` trait (`play(uri) / pause / next / prev / seek / set_volume` +
state events) with two implementations; `PLAYER_STATE`, `PlayerBar`, and all pages are
engine-agnostic.

- **`sdk::Engine`** (existing, official, Premium-only): hidden WebView + Web Playback
  SDK with its IPC queue — kept verbatim; that queue pattern is load-bearing
  (RULES §6.5).
- **`open::Engine`** (new, any account): resolves a `spotify:track:` to a direct audio
  URL via §6.7 and plays it through a local Rust audio sink (`rodio` + `symphonia`
  decode; next track pre-buffered ~10 s for gapless advance; seek = resume decode at a
  byte offset).
- **Optimistic controls are identical for both engines**: play/pause/volume flip
  `PLAYER_STATE` immediately and reconcile on the engine's state event.
- **Selection & fallback**: Settings default + automatic failover (SDK init failure /
  free account / resolver outage → other engine). Both engines log resolution success
  rate and time-to-first-audio; those numbers decide the default in the testing phase.
- Local queue model: app owns an explicit queue (`Vec<Track>` + index); shuffle is local
  (order preserved for un-shuffle); dedup by track id at ingestion via one
  `PLAYER_STATE.write_queue()` entry point (Spotufi lesson).

### 6.4 Artwork

New `src/media/images.rs`: stream download → SHA-keyed file under cache dir → `<img>`
from file path; LRU eviction (~512 MB). `image` crate already a dependency if we later
add thumbnail decoding.

### 6.5 Ad-block engine v2 (Brave-style)

Replace the radix-trie blocker with the `adblock` crate behind the same facade
(`adblock::should_block`, stats):

- `FilterSet` inputs:
  1. bundled AdGuard DNS snapshot (`assets/blocklist_cache.txt`; hosts syntax natively supported),
  2. curated network rules for Spotify ad/analytics hosts (RESEARCH §3.3),
  3. exception rules encoding `ALWAYS_ALLOW` so the whitelist lives in the engine.
- Boot path: `Engine::deserialize()` from `{cache_dir}/adblock_engine.bin` when the list
  version matches; else compile + serialize in background (fast restarts).
- Enforcement points:
  1. `client::filtered_*` → `check_network_request(Request { request_type: Xhr, .. })`,
  2. optional cosmetic layer: element-hiding CSS injected into any WebView that shows a
     live Spotify page (login/session), targeting upgrade buttons / hpto / sponsored
     items — SpotiCap tier 2 equivalent.
- Audio-ad substitution (silent-media replacement): designed but **disabled by default**
  and gated behind a Settings toggle with a ToS warning (rationale §7). Premium paths
  unaffected.
- `ADBLOCK_STATS` fed from BlockerResult matches; displayed only in Settings → Privacy.

### 6.6 Session/auth

Unchanged (works, and it's the most fragile part to churn). One addition: profile fetch
moves behind the store so cold-start paints UI from the last-known profile snapshot
before `/v1/me` returns.

### 6.7 Open-engine stream resolution (`src/streaming/`)

Pipeline adapted from Spotufi's `SpotiFlac.kt` (RESEARCH §2.3):

1. **Map** Spotify track → provider IDs via Odesli (song.link), cached by track id;
   Qobuz additionally matched by ISRC (already present in album payloads) through
   Qobuz's public search.
2. **Resolve** a stream URL trying providers in order, each returning an explicit state:
   `Success | Cooldown(503, retry_after) | NotFound | Error`.
   - *TIDAL*: public monochrome/squid.wtf Hi-Fi instances — live uptime list
     (`tidal-uptime.geeked.wtf`) cached ~5 min, merged in front of a static pool.
   - *Qobuz / Amazon*: community proxy endpoints, same contract.
   - *YouTube*: InnerTube client as final fallback (audio-only; PoToken timeouts so a
     hang can never stall track-skipping).
3. **Cache** resolved URLs (memory + disk, keyed by track id + provider, short TTL —
   stream URLs expire); repeat plays skip resolution entirely.
4. **Fetch & decode** progressively into the local sink. Metadata/artwork always come
   from Spotify, so the UX remains pure "Spotify" regardless of which engine plays.
- Every outbound request still passes the ad-block gate; resolver hosts join the
  exception list so they can't be dropped by list updates.

---

## 7. Product stance & risk notes

**No-paywall philosophy (owner decision):** every signed-in user gets the full app —
browsing, playlists, history, and full-track playback. Premium accounts get the official
Web Playback SDK; free accounts (or any situation where the SDK can't initialize) get
the open engine (§6.7), which sources audio from third-party services instead of
Spotify's gated stream. We do not tamper with Spotify's servers or tokens — we route
around the gate entirely.

Accepted trade-offs, documented rather than hidden:

- **Resolver fragility**: community backends occasionally 503 or churn. Mitigated by
  cooldown states, provider failover order, uptime-list caching, and — ultimately — by
  the fact that both engines exist and either can be made default after testing.
- **Legal posture**: metadata comes from the Spotify API under the user's own session;
  audio comes from third-party services the user requests. This mirrors the established
  Spotufi/monochrome ecosystem model. Keep endpoints configurable and avoid bundling
  secrets where possible; users remain responsible for compliance with their local laws
  and terms of service.
- **Ad-blocking scope shrinks** once free-tier playback no longer flows through
  Spotify's ad-supported stream. Engine v2 still blocks third-party trackers/telemetry
  and hides upsell UI; SpotiCap-style silent-audio substitution becomes unnecessary and
  is dropped from scope.
- **Endpoint drift**: hashed CSS classes rotate; `/api/token` now wants a TOTP (our
  WebView capture computes it page-side, so we're insulated); resolver instances churn
  (uptime-list pattern handles it). RULES.md keeps documenting the 429/pathfinder
  reality so nobody re-litigates a GraphQL migration during an outage.

## 8. Testing & verification strategy

- Unit tests stay network-free: store TTL/SWR semantics, coalescing (N callers → 1
  fetch), engine decisions (blocked/allowed/substituted), resolver state machine +
  cooldown/failover order + URL-cache expiry, settings load/save, theme constant sync.
- Existing adblock tests ported unchanged against the new facade.
- **Playback A/B harness (testing phase)**: run both engines over a fixed playlist and
  score resolution success %, time-to-first-audio, seek latency, gapless smoothness,
  audio quality, and failure modes. The winner becomes the default; the loser stays
  behind a Settings flag or is removed.
- Gates per phase: `cargo check --features desktop`, clippy zero warnings,
  `cargo test` + `cargo test --no-default-features`. Visual QA is manual by the user
  running their own `dx serve`.

## 9. Milestone ordering (summary)

1. Tokens + theme plumbing + settings store (foundation everything else skins onto)
2. App shell (top bar, rail, now-playing column)
3. Pages: Home → Library → Search → detail pages → Queue → Settings
4. Store/coalescing/prefetch/image pipeline (perf)
5. Open streaming engine (`src/streaming/`) + local audio sink — free-tier full playback
6. Ad-block engine v2
7. Testing-phase engine verdict (SDK vs open) → responsive polish + hardening + docs

Full task breakdown: `docs/ROADMAP.md`.



