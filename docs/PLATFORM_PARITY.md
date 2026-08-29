# Platform Parity — web / android / ios on the same API as desktop

Status: **in progress** — web (WASM) parity foundation landed and fully green;
remaining item is live browser validation of the Spotify session token capture
(see Phase B). Android/iOS still planned.

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

## Decisive findings (research confirms — big simplification)

- **Mobile ≈ desktop already.** `dioxus::mobile` is a re-export of
  `dioxus::desktop` (both wrap `wry` + `tao`; iOS = WKWebView, Android =
  system WebView). So the wry webview login (`webview_login.rs` + `POLL_JS`)
  and `dioxus::document::eval()` IPC work on iOS/Android **almost unchanged**
  — only the *GTK window* bits (`cfg(target_os = "linux")`) differ, and they
  are already gated.
- **The `audio` seam dissolves.** `rodio` + `cpal` + `symphonia` cover **all
  six** platforms with no per-OS backend abstraction: Linux (ALSA), macOS
  (CoreAudio), Windows (WASAPI), Android (AAudio), iOS (CoreAudio), and web
  (rodio `wasm-bindgen` → WebAudio). `media/sink.rs` needs **zero changes** on
  Android/iOS; only the web WebAudio single-thread model is a caveat.
- **The "Connect API only" limit is a feature-gating artifact, not an
  architecture.** `main.rs` gives web/mobile a trivial `dioxus::launch(App)`
  while the real backends are behind `#[cfg(feature = "desktop")]`. If we gate
  on `cfg(target_os)` / `cfg(target_arch = "wasm32")` instead, the same code
  compiles for mobile.
- **Storage:** `keyring` v4 now ships first-class Android
  (`android-native-keyring-store`, `android-keyring` — works out of the box
  with dioxus-mobile since it initializes `ndk-context`) and iOS (Keychain)
  stores under the same API. Desktop `keyring` call sites need only the right
  store selected per target.

## The seams that actually remain

| Seam | Desktop / Android / iOS (shared) | Web (WASM — the only true divergence) |
| --- | --- | --- |
| **Login** | wry webview + `POLL_JS` (already written, desktop-gated today) | the browser *is* the webview — open.spotify.com in a route; same `POLL_JS` fetch hook |
| **Audio** | `media/sink.rs` rodio/cpal/symphonia — **no changes** | WebAudio via rodio `wasm-bindgen` (`noise-suppression`/single-thread caveats) |
| **KV store** | `keyring` v4 (Keychain/KeyStore stores) | no fs — localStorage/IndexedDB (`dioxus-sdk-storage`, `use_persistent`) |
| **Disk cache/blocklist bin** | `std::fs` under `dirs` | IndexedDB / Cache API; `std::fs` gated out of WASM |
| **Ad-block thread** | `adblock` crate on std thread | WASM single-thread — sync engine call or Web Worker

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

### Phase A — Introduce the native/non-wasm seam ✅ (landed)
- **Done:** added a `native` feature (`dep:wry`) that both `desktop` and
  `mobile` enable, expressing the real boundary: native (desktop **+** mobile,
  both wry-based because `dioxus::mobile` re-exports `dioxus::desktop`) vs
  WASM (`web`). Moved the platform-agnostic `playback_sdk` bootstrap from
  `#[cfg(feature = "desktop")]` to `#[cfg(feature = "native")]`.
- **Verified:** `cargo check --features desktop` and `cargo check --features
  mobile` both compile on the native host; clippy clean for both; 79/79 tests
  pass on `--features desktop` and `--no-default-features`.
- **Key finding that scopes Phase C:** the sign-in webview (`auth/webview_login.rs`)
  and hidden SDK webview (`player/webview_bridge.rs`) are **GTK-coupled**
  (they host/upcast the wry `WebView` into a `gtk::Widget` container, gated to
  Linux + `feature = "desktop"`). So they cannot compile for mobile until
  Phase C re-hosts them on the dioxus-mobile webview surface. They are left
  desktop-only for now; mobile keeps the Connect-API fallback for that one
  SDK path until Phase C.
- Media / adblock / auth-token / open-engine logic is already platform-agnostic
  (not gated on `desktop`) and is shared as-is.

> Remaining Phase A intent (not yet done): introduce the small `src/platform/`
> seam (`Storage` keyring-store selection, `DiskCache`) for mobile — this is
> pulled forward with the Phase C webview work since both touch auth/media.

### Phase B — Web (WASM) build of the same code ✅ (foundation landed)
- **Cargo restructure:** single general `[dependencies]` block plus per-target
  blocks — `cfg(not(target_arch = "wasm32"))` (tokio-full, reqwest
  cookies+gzip+brotli+stream+rustls-tls, hickory-resolver, keyring, dirs,
  symphonia, rodio), `cfg(target_arch = "wasm32")` (tokio rt+time+sync+macros,
  reqwest json, getrandom js, wasm-bindgen, wasm-bindgen-futures, web-sys), and
  `android`/`linux` blocks. `wry`/`gtk`/`keyring`/`dirs`/`symphonia`/`rodio` are
  gated out of WASM.
- **`src/platform/` seam created:** `storage.rs` (set/get/remove bytes —
  native fs under `data_dir`, wasm localStorage base64) via `Storage`/`Element`,
  plus `spawn_background` (tokio::spawn / spawn_local) and `web_login.rs`.
- **Token store / settings / media images / streaming cache / store snapshots**
  all refactored onto the storage seam; keyring stays native-only.
- **Audio:** `media/sink_wasm.rs` — an `HtmlAudioElement` sink exposing the same
  public API as the native rodio sink (`SinkCommand`, `SinkState`,
  `global_sink`, `spawn_sink`); native symphonia/rodio gated `not(wasm32)`.
- **Ad-block:** native thread/DoH (`hickory`) vs wasm `thread_local!` inline
  engine; blocklist fetch gates the forbidden `Accept-Encoding` header to native.
- **HTTP seams:** `spotify/client.rs` + streaming providers gate
  `cookie_store`/`gzip`/`brotli`/`timeout` to native; shared `#[async_trait(?Send)]`
  `Provider` trait; `Accept-Encoding` and `Accept-Language` gated to native.
- **Login:** `auth::login()` on wasm uses `platform::web_login` — whole-tab
  redirect to `open.spotify.com`, then a credentialed
  `get_access_token` capture (same as the desktop WebView's `fetchAccessToken`).
- **Verified:** `cargo check --no-default-features --features web --target
  wasm32-unknown-unknown` and `cargo clippy` for that target both clean (0
  warnings). Desktop + mobile + headless all stay clean;  79/79 tests pass on
  desktop and no-default.
- **Validation-pending (the one runtime item):** whether Spotify's
  `open.spotify.com/get_access_token` actually permits a **credentialed
  cross-origin** fetch from the web app's own origin (CORS + the browser
  sending the `sp_dc` HttpOnly cookie). This cannot be verified from a headless
  sandbox — it requires a real logged-in browser. If Spotify denies it, the
  token capture returns a clear CORS error via the login gate and needs a
  different capture mechanism. The redirect itself is correct regardless. This
  is the sole remaining Phase B item.
- Also follow as validation: Web Playback SDK playback (`player/` dispatch) on
  wasm uses the open engine resolution; actual streaming needs the SDK/token
  path confirmed in-browser.

### Phase C — Android + iOS builds of the same code
- ✅ **SDK playback un-gated to `native`** (`player/webview_bridge.rs`,
  `playback_sdk`) so mobile runs the full app via the SDK, not the Connect-API
  shell. `main.rs` mobile entry runs `rt.block_on(bootstrap())` then
  `dioxus::launch(App)`.
- ✅ **Pure-mobile build** `cargo check --no-default-features --features mobile`
  and **Android cross-build** `cargo check --no-default-features --features
  mobile --target aarch64-linux-android` are both green (0 warnings; see
  RULES.md §6.9b for the NDK symlink trick — no `cargo-ndk` needed). iOS targets
  aren't installed on this Linux host.
- ✅ **In-app login on mobile.** `auth/webview_login.rs` is now `native`-gated
  with two hosts: GTK-packed on Linux desktop vs wry's cross-platform
  `build(&window)` on mobile / non-Linux desktop, so `open.spotify.com` opens
  INSIDE the app (fills the window) exactly like desktop — same session
  WebContext, cookies, `POLL_JS`, and same-origin token-refresh path. `auth::
  login()` and `ensure_session()` run for every native renderer (see RULES.md
  §6.9b). Runtime stacking of the login WebView over the dioxus webview on
  Android/iOS is the one item that can't be exercised in this Linux sandbox.
- Keyring store selection per OS (`android-keyring` initializes ndk-context
  itself; iOS Keychain store).

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
