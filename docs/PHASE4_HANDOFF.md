# Phase 4 Hand-off — Spotify DX (2026-08-25)

## What was completed

1. `src/spotify/store.rs` — single-flight coalescing, memory TTL (5min, FIFO-capped 256), disk stale-while-revalidate (`SWR_WINDOW` 24h), never caches errors. 7 passing unit tests (`coalesces...`, `memory_ttl...`, `errors_are_never_cached`, `follower_of_failed_fetch...`, `disk_roundtrip...`, `stale_hit...`, `fifo_cap...`).
2. `get_home()` fan-out (`tokio::try_join!`) — featured + new_releases concurrent; recommended spawned separately (best-effort). Zero-compile errors.
3. `src/media/images.rs` — SHA-keyed disk-cached artwork (`~/Library/Caches/spotify-dx/img_cache/`), 128-file LRU cap (`trim_cache()`), `load(url)` returns `Arc<Vec<u8>>`. AlbumArt (`src/ui/components/album_art.rs`) now calls `images::load()` instead of direct network fetch.
4. `PlayerState` local queue model (`enqueue`/`enqueue_many` by dedup-id, `pop_queue_head`, `set_shuffle` with snapshot restore + Fisher-Yates). Tests added (`enqueue_dedups...`, `enqueue_many...`, `pop_queue_head...`, `shuffle_then_unshuffle...`).
5. `player::enqueue()` public + `player::next()` now prefers local queue head over SDK `next()`; `player_bar.rs` shuffle onclick updated.
6. `api.rs` cleaned (`api_get_json` removed; `cached_get_json`/`live_get_json`/`pipeline_load` in use); `store::Store::global()` wired.

## Remaining Phase 4 work (not completed — document for next agent)

- Route-hover prefetch (`HoverPrefetch` >150ms dwell) for detail routes.
- Like endpoint (`PUT/DELETE /me/tracks`) — `client` has no `filtered_delete_auth` yet; `player_bar` like button is optimistic stub only.
- `PlaybackEngine` trait scaffolding (Phase 4b open-engine) — `src/media/audio.rs` uses `symphonia` 0.6 but no trait abstraction yet.

## Key conventions / gotchas

- **Never touch `target/` or start `dx serve`.** User manages `dx serve`.
- `SPOTIFY_CLIENT_ID` is NOT needed (auth via webview session).
- `store.rs` uses `parking_lot::Mutex`, `tokio::watch`, `Arc`. Errors are propagated via `Fail` clone, never cached.
- `resolve()` takes `self` by value (move into spawn for SWR background refresh). Clone `Store` (`Arc`-backed) freely via `.clone()`.
- `load_image_bytes` in `album_art.rs` is now a thin wrapper over `media::images::load()`.
- `queue_original` snapshot restored by `set_shuffle(false)`. Shuffle is deterministic (seeded PRNG) but not identity-guaranteed; unshuffle restores exactly.

## Verification commands

```bash
cargo check --features desktop          # zero errors (only expected `unused` warnings for _cacheable removed)
cargo clippy --features desktop         # clean
cargo test --features desktop store::   # 7 passing
cargo test --features desktop state::   # 7 passing (4 new queue tests)
```

## Files to read before continuing

- `docs/SYSTEM_DESIGN.md` (cache design, pipeline §6.1–6.7)
- `docs/RESEARCH.md` (open-engine provider mapping, rate-limit behavior)
- `src/spotify/store.rs` (single-flight + SWR details)
- `src/spotify/api.rs` (`cached_get_json`/`live_get_json` split, 429 backoff rules)
- `RULES.md` (discoveries: `update_if` missing on `watch::Receiver`, `load()` moves `self`, sed `&` expansion trap, `cacheable` dead param, `SlotResolution` no `Copy`)
