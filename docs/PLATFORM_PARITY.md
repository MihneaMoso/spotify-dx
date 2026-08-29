# Platform Parity — web / android / ios on the same API as desktop

Status: **planned** (not started). Tracked separately from the release pipeline.

## Goal

Today `--features desktop` is the real app: real Spotify web login, ad-block
engine, local audio sink (`rodio` + `symphonia`), OS keychain, disk cache, and
full-track playback via the open engine and/or Web Playback SDK. The `web` and
`mobile` renderers fall back to the **Connect API only**: they cannot log in
through the real Spotify page, cannot play full tracks, and share almost none
of the desktop backends.

We want `web` / `android` / `ios` to expose **the exact same API surface as
desktop** — sign-in, ad-block, sessions, full-track playback — with
platform-specific implementations only where the OS forces one.

## What differs per platform (the seams)

These are the only places a platform forces a different implementation. Keep
everything else shared.

| Seam | Desktop (today) | Web (WASM) | Android | iOS |
| --- | --- | --- | --- | --- |
| **Login webview** | `src/auth/webview_login.rs` — GTK window hosting open.spotify.com + `POLL_JS` | The browser tab itself is the webview; capture via injected fetch hook (same `POLL_JS`) | dioxus-mobile webview surface (WebViewCompat) | WKWebView via dioxus-mobile |
| **Audio output / playback** | `src/media/sink.rs` — `rodio` + `cpal` device sink + `symphonia` decode | WebAudio (no raw file write possible) | `oboe`/AAudio + `MediaCodec` (or ffmpeg) | `AVAudioEngine` + `AVAudioPlayer`/`AudioToolbox` |
| **Persistent key-value store** | OS keychain (`keyring`) + `dirs` files | no fs — needs IndexedDB/LocalStorage abstraction | `EncryptedSharedPreferences` / data dir | Keychain / UserDefaults |
| **Disk cache / images / blocklist bin** | `std::fs` under `dirs` cache dirs | IndexedDB / Cache API | app data dir | app sandbox data dir |
| **Ad-block engine thread** | `adblock` crate on a std thread + serialized `.bin` | WASM — single thread; sync engine call or Web Worker | native thread OK | native thread OK |

### Non-goals for parity
- Not shipping identical *binaries* — only an identical *API/UX*. The UI
  (`src/ui/`) and the data layer (`src/spotify/`, `src/streaming/resolver.rs`)
  are already platform-agnostic and stay shared.
- Not doing OS-native packaging (`.app`/`.msi`/store) — that's the subsequent
  mobile/web *release* task, separate from making them *build*.

## The abstraction plan (ordered, each ends green)

Every phase must leave the repo green:
`cargo check --features desktop`, `cargo clippy --features desktop` (0 warnings),
`cargo test` + `cargo test --no-default-features`.

### Phase A — Define a `platform` layer (the seams, behind one crate module)
- New `src/platform/` module with traits, each with a **desktop implementation
  (std)** that delegates to today's code, so desktop behavior is byte-identical:
  - `Storage` (persistent KV + cache dirs) — desktop = keyring + `dirs`.
  - `AudioBackend` (open a stream URL, get position/duration callbacks) —
    desktop = thin wrapper over `media::sink`.
  - `LoginWebview` — desktop = the existing GTK `LoginWebView` (keep the
    `ready`/`suspended` park + revive logic).
- Move feature-gated calls in `auth`, `player`, `adblock`, `media` to go through
  these traits. Keep `#[cfg(feature = "desktop")]` as *the std impl*.
- Verify: desktop suite still 79/79 + green clippy.

### Phase B — Web (WASM) build of the same code
- Add a `web` native impl of the traits: `Storage` → `web_sys` IndexedDB,
  `AudioBackend` → WebAudio via `web-audio-api` crate, `LoginWebview` → the
  dioxus-web window hosting the login + the same `POLL_JS` fetch hook.
- `player/mod.rs` already dispatches to open-engine; on web the open engine's
  *resolution* (Odesli/TIDAL/Qobuz/YouTube URLs) is shared — only the "sink"
  becomes WebAudio.
- Make the `web` feature compile with the full module set (today only `main.rs`
  differs). Gate GTK/wry/rodio deps so `--features web` has no Linux/GUI deps.
- Verify a `wasm32-unknown-unknown` release build succeeds (add to the CI matrix
  as an artifact build, not required to run).
- iOS/Android reusable: the `Storage`/`AudioBackend`/`LoginWebview` trait
  contract is identical; only impls differ.

### Phase C — Android + iOS builds of the same code
- `cargo-ndk` for Android; `--target aarch64-apple-ios` etc. for iOS.
- Implement `LoginWebview` and `AudioBackend` for each OS (see seam table).
- Wire dioxus-mobile entry point (already present in `main.rs`) to run the full
  app, not the Connect-only shell.

### Phase D — Mobile/web release CI
- Extend `.github/workflows/release.yml` with `wasm32` (web) and Android/iOS
  artifact jobs, reusing the same tag + `./scripts/release.sh` trigger.
- Update README + RULES.md to drop all "Connect API only" language once parity
  lands.

## Acceptance criteria
- `cargo build --release --features web --target wasm32-unknown-unknown` succeeds.
- Android APK + iOS archive build in CI.
- On every platform: real open.spotify.com login, ad-block drop counters,
  persistent session across restart, and full-track playback via the open engine.
- Zero "Connect API only" mentions remain in docs/README/MASTER_PROMPT.
