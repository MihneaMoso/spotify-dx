# Research — external projects & strategies (2026-08)

Gathered intel for the spotify-dx rework. Sources:

| Source | What it is | Relevance |
| --- | --- | --- |
| [fr0stb1rd/Spotufi](https://github.com/fr0stb1rd/Spotufi) (archived 2026-08-19) | Kotlin multi-source Android player: Spotify sync + lossless FLAC (TIDAL/Qobuz/Amazon) + YouTube fallback | Auth internals, stream resolution, caching |
| [spotihater/spoticap](https://gitlab.com/spotihater/spoticap) | Capacitor WebView wrapper around open.spotify.com with theming + ad blocking | Themed-webview strategy, tiered ad blocking |
| [brave/adblock-rust](https://github.com/brave/adblock-rust) (+ docs.rs) | Brave's native Rust ad-block engine (`adblock` crate v0.13.x) | Engine design we will adopt |
| This repo (`RULES.md`, `SPOTIFY_API_AND_PLAYBACK.md`, source) | Current spotify-dx architecture | Baseline |

---

## 1. Where spotify-dx stands today (baseline)

**What works and must be kept:**

- **Web-session auth** (`src/auth/webview_login.rs`): in-window GTK WebView loads
  `open.spotify.com`; injected poller captures the web-player access token from
  `open.spotify.com/get_access_token?reason=transport&productType=web_player` using the
  HttpOnly `sp_dc` cookie. Session cookies persist in the shared WebView data dir; token
  mirrored to OS keychain. No `SPOTIFY_CLIENT_ID`. The login WebView stays alive after
  sign-in as the *session WebView* (only origin that can refresh the token — the SDK
  WebView is null-origin and CORS-blocked).
- **Playback** (`src/player/`): hidden wry WebView runs the Web Playback SDK
  (`sdk.scdn.co/spotify-player.js`), self-fetches its token inside the WebView, forwards
  `player_state_changed` over `window.ipc.postMessage`; Rust drives it via
  `window._relay`. Premium-only (free accounts get benign `init_error`).
- **Data layer** (`src/spotify/`): single filtered reqwest client (Chrome UA); every
  request passes `adblock::should_block`; `api_get_json` handles 401-refresh-retry /
  capped 429 backoff / memory+disk 5-min TTL cache.
- **Ad block v1** (`src/adblock/`): AdGuard DNS list parsed into a reversed-label radix
  trie, O(k) ancestor lookup, hard `ALWAYS_ALLOW` whitelist for `*.spotify.com`,
  `*.spotifycdn.com`, `*.scdn.co`.
- **UI**: Dioxus 0.7 router (`Home/Search/Library/Album/Artist/ArtistTopTracks/Playlist`)
  inside `AppLayout` (side/bottom nav + player bar + toast). Token-driven CSS design
  system in `assets/main.css`; tokens mirrored in `src/ui/theme.rs`.

**Known constraints (RULES.md §6):**

- `api.spotify.com/v1` gets hard-429'd during outage windows; the real web player browses
  via `api-partner.spotify.com/pathfinder` (GraphQL) and `spclient.wg.spotify.com`, but
  **neither accepted our BQA/web-session token in testing** (401/404). Do not bet the
  data layer on pathfinder.
- Never reparent a realized WebView; never touch signals from the wry IPC thread
  (queue + drain).
- `/v1/me/player*` endpoints are Premium-gated (403).

---

## 2. Spotufi — streaming, auth, caching

### 2.1 Architecture
- Multi-module: `:app` (UI/playback/DI), `:spotify` (JVM lib: Web API client + FLAC
  resolver), `:innertube` (YouTube InnerTube client). MVVM, Compose, Media3
  `PlaybackService` foreground service with MediaSession notifications.
- Data layer dedups **every** song list by `distinctBy { it.id }` at ingestion — prevents
  LazyColumn key collisions, keeps scroll/reorder stable. Cheap lesson; worth copying for
  our track lists/queue.
- Persistence: SharedPreferences + JSON for small KV caches (lyrics, **stream URLs**,
  downloads, history, playback position).

### 2.2 Auth internals (`SpotifyAuth.kt`) — notable discovery
Spotufi fetches the internal web-player token **natively, without a WebView**:
1. `GET https://open.spotify.com/api/server-time`
2. Fetch a rotating **TOTP secret + version from a community GitHub Gist**
3. Generate 6-digit TOTP (SHA1, 30 s) over server time
4. `GET https://open.spotify.com/api/token?reason=transport&productType=web-player&totp=…&totpVer=…`
   with `Cookie: sp_dc=…`

Rejects `isAnonymous == true`. Login URL forces the password form:
`accounts.spotify.com/en/login?continue=…&method=password&allow_password=1`.

> **Assessment:** our WebView capture is more robust (no third-party gist dependency).
> Keep ours; record this as a researched fallback. The TOTP requirement shows Spotify is
> hardening `/api/token` — if plain cookie GETs ever die, our WebView flow still works
> because the page itself computes the TOTP.

### 2.3 Multi-source streaming (`SpotiFlac.kt`) — how they get playable URLs
- Resolve Spotify track → provider IDs via **Odesli (song.link)**; Qobuz matched by
  **ISRC** via signed public search.
- Ask community proxy servers (`/api/dl`) for a FLAC URL; TIDAL via public
  "monochrome/squid.wtf" Hi-Fi API instances, with a **live uptime list** merged in front
  of a static instance pool, cached a few minutes.
- Fallback chain: TIDAL → Qobuz → Amazon → YouTube (InnerTube/NewPipeExtractor, with
  PoToken timeouts so background resolution can't hang).
- Explicit result states: `Success | Cooldown(503) | NotFound | Error`.

> **Assessment:** dependent on volunteer proxies with real latency variance — mitigated
> by exactly the patterns this code demonstrates: live uptime lists, cooldown-aware
> failover, and persistent URL caches. **Adopted** (owner decision 2026-08) as the open
> playback engine; implementation plan in SYSTEM_DESIGN §6.7.

### 2.4 Caching lessons
- **Stream URL cache (memory + persistent)**: skip re-resolution on repeat plays.
  Analogues for us: artwork disk cache + API snapshot cache.
- History/stats persist locally and paint instantly on launch — no network needed for
  first frame of history.

### 2.5 Data-layer breakthrough: Spotify's internal GraphQL (`api-partner.spotify.com`)

**Spotufi does NOT use `api.spotify.com/v1` for library/browse/home data at all.**
Its entire data layer (playlists, library tracks/albums/artists, search, home feed,
playlist detail, artist overview) runs on Spotify's internal **GraphQL persisted-query
API**:

```
POST https://api-partner.spotify.com/pathfinder/v2/query
```

Key facts (verified against the Spotufi source, `Spotify.kt` + `SpotifyHashProvider.kt`):

- **Headers**: `Authorization: Bearer <web-player token>`,
  `app-platform: WebPlayer`, `Origin: https://open.spotify.com`,
  `Referer: https://open.spotify.com/`, plus a rotating Chrome UA. Content-Type JSON.
- **Body**: `{ variables: {...}, operationName: "...", extensions: { persistedQuery: { version: 1, sha256Hash: "<hex>" } } }`
- **The token is the SAME web-player token** our app already captures from
  `open.spotify.com/api/token`. Spotufi fetches it natively via TOTP; we get it from
  the session WebView. Either way it's a `web_player` access token that works.
- **Persisted-query hashes are the load-bearing secret.** Spotify rotates them, and we
  must ship current ones. Spotufi's hardcoded hashes (SHA-256) as of 2026-08:
  - `libraryV3` (playlists/albums/artists): `973e511ca44261fda7eebac8b653155e7caee3675abb4fb110cc1b8c78b091c3`
  - `fetchPlaylist` (playlist detail + tracks): `346811f856fb0b7e4f6c59f8ebea78dd081c6e2fb01b77c954b26259d5fc6763`
  - `fetchLibraryTracks` (liked songs): `087278b20b743578a6262c2b0b4bcd20d879c503cc359a2285baf083ef944240`
  - `searchDesktop`: `4801118d4a100f756e833d33984436a3899cff359c532f8fd3aaf174b60b3b49`
  - `home`: `23e37f2e58d82d567f27080101d36609009d8c3676457b1086cb0acc55b72a5d`
  - `profileAttributes`: `53bcb064f6cd18c23f752bc324a791194d20df612d8e1239c735144ab0399ced`
  - `getAlbum`: `b9bfabef66ed756e5e13f68a942deb60bd4125ec1f1be8cc42769dc0259b4b10`
  - `queryArtistOverview`: `5b9e64f43843fa3a9b6a98543600299b0a2cbbbccfdcdcef2402eb9c1017ca4c`
  - `queryWhatsNewFeed`: `3b53dede3c6054e8b7c962dd280eb6761c5d1c82b06b039f4110d76a62b4966b`
  - `addToLibrary`/`removeFromLibrary`: `7c5a69420e2bfae3da5cc4e14cbc8bb3f6090f80afc00ffc179177f19be3f33d`
  - `addToPlaylist`/`removeFromPlaylist`/`moveItemsInPlaylist`: `47b2a1234b17748d332dd0431534f22450e9ecbb3d5ddcdacbd83368636a0990`
- **Hash rotation handling**: on `412 PersistedQueryNotFound`, re-query with a known
  "previous" hash; if that fails, trigger a remote hash refresh (Spotufi pulls a
  community-maintained JSON registry). This is defensive — ship the current hashes and
  re-check periodically.
- **`variables` for `libraryV3` (playlists)**:
  ```json
  { "filters": ["Playlists"], "order": null, "textFilter": "",
    "features": ["LIKED_SONGS","YOUR_EPISODES_V2","PRERELEASES","EVENTS"],
    "limit": 50, "offset": 0, "flatten": true, "expandedFolders": [],
    "folderUri": null, "includeFoldersWhenFlattening": false }
  ```
- **GQL response shape**: track artists are nested under `artists.items[].profile.name`
  (or `.profile`), albums under `albumOfTrack`, images under `images/sources[].url`.
  Track URIs may be on a wrapper `_uri`. Playlist tracks live at
  `data.playlistV2.content.items[].itemV2.data`.
- **429 handling**: GQL honors `Retry-After`, retries ~3× up to a few seconds each,
  then surfaces a rate-limit error. Pathfinder is far less rate-limited than `/v1`.
- **No single-track GQL op** — Spotufi fetches single tracks via REST `/v1`
  (`tracks/{id}`), which is hard-429'd for us. **We get single-track metadata via
  the `searchDesktop` GQL op by searching the exact track URI** (Spotify matches
  full URIs). Searches return the first `Track` whose id matches.
- **Playback should reuse in-hand metadata, not `/v1`.** Correction to the earlier
  plan: instead of calling `/v1/tracks/{id}` before playback (429), the UI passes
  the full `Track` the user clicked straight to the open engine
  (`player::launch_track`). Only URI-only entry points re-fetch, via GQL
  `searchDesktop`. This means the open-engine path never needs `/v1`.

> **Assessment (supersedes §1 "pathfinder didn't accept our token"):** The earlier
> rejection was almost certainly a transient 429/hardening window or a missing
> `app-platform`/Origin header — Spoticap/Spoofy-class clients and Spotufi all drive
> their entire data layer through pathfinder with the plain web-player token. **Adopted
> (2026-08)**: replace all `/v1` library/browse/home/search reads with pathfinder GQL.
> Keep `/v1` only for the few endpoints with no GQL equivalent (top tracks/artists,
> recommendations). This sidesteps the `/v1` 429 outage entirely. Confirmed working
> in-app: home feed, user playlists, liked songs, playlist detail all load via GQL.


---

## 3. SpotiCap — themed WebView + tiered ad blocking

### 3.1 Strategy
Load **the real open.spotify.com** in a WebView with a **desktop Chrome UA override**
(forces the full web player instead of the mobile browse experience). Config:
`server.url = https://open.spotify.com`; `allowNavigation` for `*.spotify.com`,
`accounts.spotify.com`, `*.scdn.co`, `*.spotifycdn.com`.

### 3.2 Theming that survives Spotify renames ("DOM tagger")
- Spotify ships hashed CSS classes that change ~every deploy. `spoticap.js` finds
  elements via **stable attributes** (`data-testid`, `aria-label`, element ids,
  structural position) and tags them with its own stable `sc-*` classes.
- Theme CSS (`themes/default.css`, `amoled.css`, `nord.css` + `manifest.json`) targets
  only `sc-*` classes → themes don't rot when Spotify redeploys.
- A `MutationObserver` re-tags lazily-loaded SPA content; second pass at +2 s;
  history.pushState/popstate hooked for navigation/back-button handling.

> **Assessment:** this is the themed-webview route — we are NOT taking it (native Dioxus
> UI stays). The transferable lesson: `data-testid` attributes are Spotify's stable DOM
> vocabulary; use them in any injected CSS and as naming inspiration for our components.

### 3.3 Ad blocking — three tiers (`SpotiCapPlugin.java` + `www/js/adblock.js`)
1. **Native request interception (`shouldInterceptRequest`)**:
   - *Analytics domains* → instant empty HTTP 200 with CORS `*`:
     `doubleclick.net`, `googlesyndication.com`, `fastly-insights.com`, `sentry.io`,
     `googleadservices.com`.
   - *Audio-ad URL patterns* → probe the real request's `Content-Type`; if `audio/mpeg`,
     **replace the response body with a bundled silent MP3**:
     `akamaized.net/audio/`, `scdn.co/audio/`, `scdn.co/mp3-ad/`,
     `spotifycdn.com/audio/`, `amillionads.com`, `2mdn.net`, `adxcel.com`,
     `adstudio-assets.scdn.co`.
   - *Whitelist passthrough*: `podz-content`, `gew4-spclient` (real content CDNs whose
     URLs share shapes with ad URLs).
2. **Injected CSS element-hiding** (MutationObserver re-injection): upgrade buttons
   (`[class*="UpgradeButton"]`, `a[href*="/premium/"]`), ad containers
   (`.main-leaderboardComponent-container`, `div[data-testid*="hpto"]`,
   `div[data-testid*="ad-"]`, `[data-testid="sponsored-item"]`), ad iframes
   (`iframe[src*="ads"|"doubleclick"|"googlesyndication"]`).
3. **Runtime-level probe (experimental, `adblock-probe.js`)**: reach into Spotify's
   webpack chunk registry, find the protobuf transport prototype, patch
   `callSingle`/`callStream`, then drive internal gRPC-web services directly:
   `spotify.ads.esperanto.proto.Slots.getSlots()` → `clearAllAds({slotId})`, plus
   `Testing.addPlaytime({seconds: -100000000000})` (resets playtime-based ad cadence).

> **Assessment:** tiers 1+2 map cleanly onto our capabilities (reqwest gate ≈ network
> interception; injected CSS ≈ cosmetic filtering). Tier 3 is fragile webpack spelunking
> tied to specific module indexes — never ship; at most a research flag later.

### 3.4 Other tricks
- JS polls player DOM → `JavascriptInterface` bridge → foreground service → MediaSession
  lock-screen controls (deduped on title/artist/isPlaying changes).
- Player control = `document.querySelector('button[data-testid="…"]').click()` — again,
  `data-testid` as the stable handle.

---

## 4. Brave's ad-blocking strategy (adblock-rust)

Why Brave built it: extension blockers (uBlock Origin) run late in JS and cost
memory/CPU per frame. Brave embeds a **native Rust engine** in the browser process.

### 4.1 Engine shape (`adblock` crate 0.13.x)
- **`FilterSet`** collects lists (ABP syntax, uBO syntax, hosts syntax) →
  **`Engine::new_with_filter_set(set)`** compiles rules into optimized lookup structures.
  The engine is immutable after construction — updates = build a fresh engine and swap.
- **Network blocking**: `Engine::check_network_request(&Request { url, source_url,
  request_type }) -> BlockerResult` — hash/token-based matching over compiled tables, not
  linear scans. Supports `$script/$image/$xhr/$redirect` etc., exceptions, and hosts-file
  syntax natively.
- **Cosmetic filtering**: `url_cosmetic_resources(url)` returns per-page CSS + scriptlets
  before load; `hidden_class_id_selectors(classes, ids, exceptions)` handles dynamically
  appearing elements (Brave's answer to "pages change under you").
- **Resource replacements / redirection**: blocked requests can be answered with bundled
  placeholder resources (empty scripts, transparent media) or uBO-compatible scriptlets
  instead of hard-failing — avoids breakage.
- **Fast restarts**: `serialize()/deserialize()` binary format — compile once, reload
  instantly afterwards.
- **Memory/perf engineering**: regexes pooled behind a `RegexManager` with discard
  policies; `single-thread` feature trades Send/Sync for speed+size; Brave's Jan-2026
  overhaul cut engine memory ~75% via flatter data layouts/serialization.
- Tags (`enable_tags/disable_tags`) flip rule groups at runtime without a rebuild.

### 4.2 Enforcement points: Brave vs. spotify-dx

| Brave | spotify-dx equivalent |
| --- | --- |
| Browser network-stack interception | `spotify::client::filtered_*` reqwest gate on every outbound call |
| Per-frame stylesheet injection | Cosmetic CSS into the login/session WebView when it shows live Spotify pages |
| Redirect resources | Local substitutes for blocked media (silent-audio pattern; flagged off by default) |
| Serialized engine cache | Same: compile filter set once, `deserialize` on boot |

---

## 5. Synthesis — what we take from each

| Need | Take from | Apply as |
| --- | --- | --- |
| Spotify layout vocabulary | SpotiCap (lives inside the real UI) | Page/section inventory + `data-testid` naming inspiration (SYSTEM_DESIGN §4) |
| Theme system that doesn't rot | SpotiCap `sc-*` tagger | Not needed for native UI; pure CSS-variable themes instead |
| Fast layered caching | Spotufi stream/history caches | Two-tier API cache + artwork disk cache + instant local-history paint |
| List stability | Spotufi dedup-by-id | Dedup tracks at ingestion into queue/page state |
| Ad/tracker blocking | Brave engine + SpotiCap tiers 1–2 | Adopt `adblock` crate Engine; keep reqwest gate; optional WebView cosmetic layer; serialized engine; silent-substitute scaffold gated OFF |
| Token acquisition fallback | Spotufi TOTP flow | Documented fallback only; keep WebView capture |

**Owner decision (2026-08, supersedes the original scoping):** multi-source FLAC/YouTube
streaming IS adopted — both playback engines get built and compared during testing, and
free accounts get full-track playback (no-paywall philosophy; SYSTEM_DESIGN §6.3/§6.7).
Still out of scope: webpack/esperanto probing in shipped code, replacing the native
Dioxus UI with a wrapped webview.



