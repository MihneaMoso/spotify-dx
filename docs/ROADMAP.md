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

## Phase 4 — Data-layer performance (completed: store, parallel home, images cache, local queue model, player enqueue/next)
- [ ] `src/spotify/store.rs`: memory LRU+TTL cache, disk snapshots, stale-while-revalidate, in-flight coalescing; port `api_get_json` on top; unit tests (coalescing counts, SWR returns stale then refreshes).
- [ ] Parallelize `get_home()` fan-out with `futures::join_all`.
- [ ] Route-hover prefetch (>150 ms dwell) for detail routes.
- [ ] Optimistic play/pause/volume in `PLAYER_STATE`; reconciliation on `player_state_changed`.
- [ ] Local queue model with dedup-by-id ingestion + local shuffle/unshuffle.
- [ ] `src/media/images.rs`: disk-cached artwork (SHA-keyed), LRU eviction, placeholder gradients; wire `AlbumArt` to it.

## Phase 4b — Open streaming engine (free-tier full playback)
- [ ] Define `PlaybackEngine` trait; refactor existing player dispatch behind it with zero behavior change (SDK path must stay green).
- [ ] `src/streaming/resolver.rs`: provider trait with explicit states `Success | Cooldown(503, retry_after) | NotFound | Error`; Odesli track→provider-id mapping cache.
- [ ] TIDAL provider: live uptime-list cache (~5 min TTL) merged over static monochrome/squid.wtf instance pool; Qobuz-by-ISRC + Amazon proxy providers; YouTube InnerTube fallback (audio-only, PoToken timeouts so hangs can't stall skipping).
- [ ] Stream-URL cache (memory + disk, keyed track id + provider, short TTL since URLs expire).
- [ ] `src/media/audio.rs`: rodio/symphonia sink — progressive fetch, ~10 s pre-buffer of next track, seek, gapless advance; wire into `open::Engine`.
- [ ] Engine selection in Settings + automatic fallback (free account / SDK init failure / resolver outage); per-engine metrics logged (resolution success rate, time-to-first-audio) to feed the Phase-7 verdict.
- [ ] Network-free unit tests: resolver state machine, failover ordering, cache expiry, engine dispatch.

## Phase 5 — Ad-block engine v2
- [ ] Port blocklist loading into `adblock::FilterSet` (AdGuard snapshot + curated Spotify ad/analytics rules + `ALWAYS_ALLOW` as exception rules); delete radix_trie/hickory deps if fully superseded.
- [ ] Serialize/deserialize engine cache at `{cache_dir}/adblock_engine.bin`; background recompile on list-version bump.
- [ ] Swap `client::filtered_*` gate to `check_network_request` (facade `adblock::should_block` signature preserved); port existing unit tests.
- [ ] Feed `ADBLOCK_STATS` from BlockerResult; move stats display into Settings → Privacy.
- [ ] Optional (flagged OFF, ToS warning): cosmetic CSS injection into login WebView (upgrade buttons/hpto/sponsored selectors from RESEARCH §3.3); audio-ad substitution scaffold disabled.
- [ ] Bench test: ≥100k lookups < 1 s (reuse existing perf test shape).

## Phase 6 — Polish & hardening
- [ ] Keyboard focus states, reduced-motion media query, scrollbar styling pass.
- [ ] Error/empty states per page (rate-limit banner behavior preserved — RULES §6).
- [ ] Audit: no signal writes off IPC thread; no WebView reparenting; no `dx serve` interference.
- [ ] Update `README.md`, `SPOTIFY_API_AND_PLAYBACK.md` (pipeline changes), `RULES.md` (new modules: store.rs, settings.rs, media/images.rs; adblock crate notes; theme attr mechanism).
- [ ] Final gates: clippy clean, both test suites green, cold-boot log review (no repeated fetches, engine deserialized not compiled).

## Definition of done (whole rework)
Home/Library/Search paint from cache with zero spinners on warm start; theme switch is
instant and persists; every outbound request passes the engine; playback controls respond
optimistically; **full-track playback works on free accounts via the open engine**, and
the testing-phase verdict between the SDK and open engines is recorded with metrics; all
verification commands green; docs updated.
