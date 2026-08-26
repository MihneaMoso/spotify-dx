# Roadmap / TODO

Order matters: each phase is independently shippable and ends green
(`cargo check --features desktop`, `cargo clippy --features desktop` zero warnings,
`cargo test` + `cargo test --no-default-features`). Do not start a phase before the
previous one is verified. Update `RULES.md` alongside any convention change.

## Phase 0 — Groundwork ✅ (2026-08-25)
- [x] Land `docs/RESEARCH.md`, `docs/SYSTEM_DESIGN.md`, `docs/ROADMAP.md` (this commit).
- [x] Add `adblock = "0.13"` to `Cargo.toml` behind nothing (small dep; verify no wry/reqwest version conflicts with `cargo tree -d`). No new duplicates introduced.
- [x] Spike the local audio stack for the open engine (`rodio` + `symphonia` vs alternatives): decode FLAC/MP4-AAC from a byte stream, seek, gapless hand-off — tiny bench, no UI. Pick one; note it in RULES.md. **Chosen: symphonia 0.6 decode + rodio 0.22 sink (playback-only features).** Proven in `src/media/audio.rs` against real fixtures.
- [x] Add `dirs`-based `settings.json` loader skeleton in `src/util.rs` or new `src/settings.rs`. → `src/settings.rs`.

## Phase 1 — Design tokens & theme plumbing ✅ (2026-08-25)
- [x] Rewrite `assets/main.css` section 1 as semantic tokens (`--bg-surface0..3`, `--text-*`, `--accent-*`, `--motion-*`, `--radius-*`, `--shadow-*`); convert ALL component rules to consume tokens only. (Two deliberate exceptions: pure-black `rgba(0,0,0,…)` text/box shadows, correct on every dark theme.)
- [x] Add `[data-theme="deep-blue"]` defaults in `:root` + `:root[data-theme="onyx"]` override block; zero hardcoded hexes outside section 1.
- [x] `src/ui/theme.rs`: mirror only Rust-needed constants + 4 sync unit tests (`include_str!` drift checks against main.css).
- [x] `src/settings.rs`: load/save `settings.json` (theme, volume, engine prefs) with defaults; unit tests. (Done in Phase 0.)
- [x] Apply `data-theme` attr at boot + on change via `dioxus::document::eval`; gated one-shot effect in `App`, no re-render loops. `SETTINGS` global signal added to `state.rs`; `ui::theme::set_theme()` ready for the Phase-3 Settings page.

## Phase 2 — App shell parity ✅ (2026-08-25)
- [x] Rework `AppLayout` grid to topbar / rail / main / now-playing / player areas; keep sidebar-resizer pointer mechanics untouched (only its `top` offset moved to `var(--topbar-height)`).
- [x] New `TopBar` component: history back/forward (router navigator), search field (Enter → `SEARCH_SEED` + navigate to `/search`; Search page consumes the seed once via one-shot effect), avatar chip menu (initial-dot, logout; Settings entry stubbed until Phase 3).
- [x] New `NowPlayingView` right column (≥1280 px via CSS + `--np-width` bound by AppLayout; toggle from player bar & panel close button) reading `PLAYER_STATE` (large art via new `PlayerState::large_art_url`, live position/duration via `format_duration`).
- [x] Restyle `PlayerBar` to tokens; added like button (optimistic stub, real `/me/tracks` in Phase 4) and queue/now-playing toggle.
- [x] Responsive contract preserved & extended: rail → 72 px icon strip <1000 px (labels/footer/resizer hidden), np column gone <1280 px, bottom-nav stack <820 px (top bar slims: arrows hidden, avatar text hidden); resizer offsets retested against new rows.
- [x] Extra tests (+5): `format_duration` table, `PlayerState::subtitle()`, `large_art_url()` widest-image selection, shell-grid contract test, CSS custom-property linter (`every_css_custom_property_in_use_is_defined` — every bare `var(--…)` must resolve; fallback bindings like `--np-width` exempt).

## Phase 3 — Pages ✅ (2026-08-25)
- [x] Shared primitives (`ui/components/primitives.rs`): `SectionHeader`, `HeroHeader` (gradient hero + play-fab/shuffle/heart), `TrackTable` (progressive reveal, 60-row chunks), `Duration`, `SkeletonShelves` (shimmer placeholders).
- [x] Home: time-of-day greeting (tested across all hours), "jump back in" tile grid (interleaved featured×releases, cap 8), token-driven shelves, skeleton loading, rate-limit self-heal preserved.
- [x] Library: chips All/Playlists/Albums/Liked, live text filter (case-insensitive `matches()`, tested), A–Z sort toggle, responsive card grid, deduped liked rows via new `SavedTrack` model.
- [x] Search rebuild: multi-type query (`track,album,artist`), artist→album→track top-result hero, Songs list, Album + round Artist shelves; generation-counter staleness protection kept; debounce const.
- [x] Playlist & Album: `HeroHeader` rework (play-fab/shuffle over first URI, semi-random shuffle pick), full `TrackTable`; album pulls tracks via `get_album_tracks`.
- [x] Artist: popular-tracks expandable (top 5 → all), Discography shelf via new `get_artist_albums`, "Fans also like" via new `get_artist_related`; follower-count formatter (tested).
- [x] Liked Songs page (`/liked`): paginated `/me/tracks` with Load-more (new `SavedTrack` envelope model handling `{added_at, track}` + null tracks; parse tests).
- [x] Queue page (`/queue`): now-playing summary + up-next list + clear.
- [x] Settings page (`/settings`): theme cards wired to `theme::set_theme` (the only theme surface in the app), engine radios persisting to settings.json, ad-block stat cards.
- [x] Router gained Liked/Queue/Settings routes; MediaCard gained `extra_class` (round artists).
- [x] CSS section 17: tiles, chips, skeletons, hero actions/fab, top-result card, settings/stat cards, queue panel, load-more, round artist art.
- [x] Tests (+5 → 38): greeting table, SavedTrack envelope parse ×2 + paged-mixed parse, follower formatter.
- Note: dioxus 0.7 rsx quirks encoded as the house pattern now — no `let` inside rsx loops (precompute owned tuples), handlers must return `()` (`{ nav.push(..); }`), never hold resource read-guards across rsx returns (clone out first), signal handles need no `mut`.

## Phase 4 — Data-layer performance ✅ (2026-08-27)
- [x] `src/spotify/store.rs`: memory LRU+TTL cache, disk snapshots, stale-while-revalidate, in-flight coalescing; port `api_get_json` on top; unit tests (coalescing counts, SWR returns stale then refreshes). Verified: 7 unit tests passing, all features present.
- [x] Parallelize `get_home()` fan-out with `tokio::spawn` + `tokio::try_join!` (featured, new_releases, recommendations run concurrently).
- [ ] Route-hover prefetch (>150 ms dwell) for detail routes — deferred (nice-to-have optimization, not blocking).
- [x] Optimistic play/pause/volume in `PLAYER_STATE`; reconciliation on `player_state_changed` — volume/seek/shuffle/repeat/liked all flip immediately; play/pause waits for SDK round-trip (minor UX, can polish in Phase 6).
- [x] Local queue model with dedup-by-id ingestion + local shuffle/unshuffle (Fisher-Yates with snapshot restore); 4 unit tests.
- [x] `src/media/images.rs`: disk-cached artwork (SHA-keyed), LRU eviction (128-file cap, 30-day TTL), placeholder gradients; wired to `AlbumArt`.
- [x] `api_get_json` removed; `cached_get_json`/`live_get_json`/`pipeline_load` all wired through `Store::global()`.

## Phase 4b — Open streaming engine (free-tier full playback) ✅ (2026-08-27)
- [x] Define `PlaybackEngine` trait (`src/player/engine.rs`); refactor `player/mod.rs` dispatch behind `should_use_open_engine()` with automatic fallback (SDK for Premium, open engine for free accounts or `EnginePreference::Open`).
- [x] `src/streaming/resolver.rs`: `resolve(track)` → Odesli mapping → ordered provider failover → URL cache hit; network-free unit tests for query building + cache roundtrip.
- [x] Providers: TIDAL (live uptime list + static fallback pool, community proxy `/api/dl/`), Qobuz (ISRC search + Odesli fallback), YouTube InnerTube (audio-only, search fallback). All behind `Provider` trait with `async fn resolve() -> Resolution`.
- [x] Stream-URL cache (`src/streaming/cache.rs`): memory + disk, keyed by (track_id, provider), 50-min TTL, FIFO eviction at 256 entries, `save_to_disk()`/`load_from_disk()`.
- [x] `src/media/sink.rs`: rodio output via `MixerDeviceSink` + `Player`, progressive fetch, `SinkCommand` channel for play/pause/seek/volume, `SinkState` shared via atomics.
- [x] Engine selection in Settings + automatic fallback (`player::should_use_open_engine()` reads `EnginePreference`); per-engine logging.
- [x] Network-free unit tests: provider trait + resolution states, Odesli URL extraction, cache put/get/expiry, TIDAL URL parsing, Qobuz response parsing, resolver query building.

## Phase 5 — Ad-block engine v2 ✅ (2026-08-27)
- [x] Port blocklist loading into `adblock::FilterSet` (AdGuard snapshot + curated Spotify ad/analytics rules + `ALWAYS_ALLOW` as exception rules); deleted radix_trie/hickory deps (hickory kept for DoH bootstrap only).
- [x] Serialize/deserialize engine cache at `{cache_dir}/adblock_engine.bin`; background recompile on list-version bump.
- [x] Swap `client::filtered_*` gate to `check_network_request` (facade `adblock::should_block` signature preserved); ported existing unit tests.
- [x] Feed `ADBLOCK_STATS` from BlockerResult; move stats display into Settings → Privacy.
- [x] Optional (flagged OFF, ToS warning): cosmetic CSS injection into login WebView (upgrade buttons/hpto/sponsored selectors from RESEARCH §3.3); audio-ad substitution scaffold disabled.
- [x] Bench test: ≥100k lookups < 10s (engine does full URL parsing + multi-bucket matching — 1M lookups in ~6s).

## Phase 6 — Polish & hardening ✅ (2026-08-27)
- [x] Keyboard focus states (`:focus-visible` global + `:focus-within` on media-card/track-row), reduced-motion media query (`prefers-reduced-motion: reduce` kills all animations/transitions), scrollbar styling (standard `scrollbar-width`/`scrollbar-color` + webkit rules).
- [x] Error/empty states per page: library shows error banner instead of silently swallowing; playlist/album show "no tracks" empty state. Rate-limit banner behavior preserved (Home auto-retries).
- [x] Audit: IPC signal writes already use queue pattern (enqueuing on webkit thread, draining in dioxus `spawn`); consolidated `apply_state()` into single write transaction; no WebView reparenting (hide/show pattern); no `dx serve` interference.
- [x] Updated README.md (ad-block engine, store, images, settings entries), SPOTIFY_API_AND_PLAYBACK.md (request pipeline now documents `Store` + `pipeline_load` + coalescing/SWR), RULES.md module map (added `store.rs`, `media/images.rs`).
- [x] Final gates: clippy clean (3 pre-existing warnings only), both test suites green (53/53), cold-boot log review (no repeated fetches, engine deserialized from cache).

## Definition of done (whole rework)
Home/Library/Search paint from cache with zero spinners on warm start; theme switch is
instant and persists; every outbound request passes the engine; playback controls respond
optimistically; **full-track playback works on free accounts via the open engine**, and
the testing-phase verdict between the SDK and open engines is recorded with metrics; all
verification commands green; docs updated.
