//! Versioned JNI bridge between the native core and the Kotlin Android app.
//!
//! `docs/KOTLIN_MIGRATION.md` §6 is the contract; this module is its
//! implementation. Rules enforced here:
//!
//! * **Narrow and versioned.** Every crossing is an explicit `extern "C"`
//!   function named `Java_com_spotifydx_app_CoreBridge_<method>` (Kotlin
//!   `CoreBridge` companion `@JvmStatic external fun`). [`BRIDGE_VERSION`] is
//!   negotiated at startup; Kotlin refuses to proceed on mismatch.
//! * **Boring types only.** Strings (UTF-8), ints, booleans, byte arrays.
//!   Structured data crosses as JSON text using the core's existing tolerant
//!   models. Every `String` return is an `{"ok", "code", "message", "data"}`
//!   envelope except the two cheap-sync calls (`bridgeVersion`,
//!   `shouldBlockUrl`).
//! * **No blocking calls on the interface thread.** Kotlin invokes blocking
//!   calls from `Dispatchers.IO`; cheap-sync calls may run anywhere. Long
//!   work completes inline (the calling thread blocks) or, once §6.3 push
//!   listeners land, via [`poll_events`].
//! * **No `unsafe`.** Only the safe `jni` 0.21 API is used, so the crate stays
//!   `#![forbid(unsafe_code)]`-clean. Each entry point is panic-guarded so a
//!   Rust panic becomes an error envelope instead of unwinding into the JVM.
//! * **No dioxus runtime dependence.** The bridge runs headless (no renderer,
//!   no `native` feature), and dioxus `GlobalSignal`s panic outside a runtime.
//!   Implemented surfaces therefore call only signal-free core paths
//!   (settings/profile file CRUD, adblock engine, updater network + staging,
//!   token-store file). Signal-entangled services (session orchestration,
//!   data fetchers, playback control) expose their exact §6.1 signatures but
//!   return `PHASE_2_PLUS` until their phase lands — Kotlin is written against
//!   the final contract from day one.

/// Data-surface exports (Phase 3 + resolveStream + PHASE_2_PLUS stubs).
/// Moved out verbatim; symbol names and envelopes unchanged.
#[path = "bridge_data.rs"]
mod bridge_data;
/// SDK-transport + lyrics exports (Phase 5). Moved out verbatim.
#[path = "bridge_sdk.rs"]
mod bridge_sdk;

use jni::objects::{JByteArray, JClass, JObject, JString};
use jni::sys::{jboolean, jint};
use jni::JNIEnv;
use std::panic::AssertUnwindSafe;
use std::sync::{Mutex, OnceLock};

use crate::spotify::{client, models};
use crate::{adblock, auth, profile, settings, updater, util};

/// Bridge schema version, negotiated at startup (`initCore` refuses nothing —
/// Kotlin checks this FIRST via `bridgeVersion` and aborts with a diagnostic
/// on mismatch, never undefined behavior). Additive changes only within a
/// major version.
pub const BRIDGE_VERSION: jint = 1;


/// Full-screen sign-in address (return parameter points at the web player, so
/// an already-signed-in user bounces straight through). Owned by the core so
/// the Kotlin gate asks `beginLogin` instead of hardcoding it.
const SIGN_IN_URL: &str =
    "https://accounts.spotify.com/en/login?continue=https%3A%2F%2Fopen.spotify.com%2F";

/// Freshness skew shared with `session::ensure_token`: a token within 60s of
/// expiry counts as stale.
const TOKEN_SKEW_MS: u64 = 60_000;

fn now_ms() -> u64 {
    chrono::Utc::now().timestamp_millis().max(0) as u64
}

// ---------------------------------------------------------------------------
// Bridge-owned headless state (mirrors, never dioxus signals)
// ---------------------------------------------------------------------------

/// Minimal session mirror: the Kotlin login page owns capture (see
/// `LoginWebView`), and reports captures here via `notifySession`. The gate
/// reads it via `sessionStatus` — same boolean semantics as `AUTH_STATE`.
#[derive(Debug, Clone, Default)]
struct BridgeSession {
    authenticated: bool,
    access_token: Option<String>,
    expires_at_ms: u64,
    user_json: Option<String>,
}

static SESSION: OnceLock<Mutex<BridgeSession>> = OnceLock::new();
static EVENTS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();

/// Last `checkForUpdates` result, kept so `downloadUpdate` needs no args.
static LAST_CHECK: OnceLock<Mutex<Option<updater::ReleaseInfo>>> = OnceLock::new();

/// Bridge-side updater status text (mirrors what the dioxus UI reads from
/// `UPDATE_STATUS`, which the headless core cannot touch — no runtime).
static UPDATE_STATUS: OnceLock<Mutex<String>> = OnceLock::new();

fn session() -> &'static Mutex<BridgeSession> {
    SESSION.get_or_init(|| Mutex::new(BridgeSession::default()))
}

fn events() -> &'static Mutex<Vec<String>> {
    EVENTS.get_or_init(|| Mutex::new(Vec::new()))
}

/// True while the initCore bootstrap worker (adblock/auth restore) is still
/// running. `sessionStatus` reports it so the Kotlin settled-gate can tell
/// "not yet restored" apart from "signed out".
static BOOTSTRAP_RESTORING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

fn set_bootstrap_restoring(v: bool) {
    BOOTSTRAP_RESTORING.store(v, std::sync::atomic::Ordering::SeqCst);
}

fn push_event(kind: &str, payload_json: &str) {
    let mut q = events().lock().unwrap_or_else(|e| e.into_inner());
    if q.len() < 64 {
        q.push(crate::util::json_event(kind, payload_json));
    }
}

fn last_check() -> &'static Mutex<Option<updater::ReleaseInfo>> {
    LAST_CHECK.get_or_init(|| Mutex::new(None))
}

fn update_status() -> &'static Mutex<String> {
    UPDATE_STATUS.get_or_init(|| Mutex::new("No update check has run yet.".into()))
}

fn set_update_status(s: String) {
    *update_status().lock().unwrap_or_else(|e| e.into_inner()) = s;
}

/// Dedicated Tokio runtime for blocking bridge calls. Separate from any
/// renderer runtime (none exists in the Kotlin app); two threads are plenty —
/// bridge calls are thin wrappers over the core's own background workers.
pub(crate) fn rt() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("spotifydx-bridge")
            .enable_all()
            .build()
            .expect("bridge tokio runtime")
    })
}

// ---------------------------------------------------------------------------
// JSON envelopes
// ---------------------------------------------------------------------------

pub(crate) fn ok_data(data_json: &str) -> String {
    format!("{{\"ok\":true,\"data\":{data_json}}}")
}

pub(crate) fn ok_str(s: &str) -> String {
    ok_data(&serde_json::Value::String(s.to_string()).to_string())
}

pub(crate) fn err(code: &str, message: impl std::fmt::Display) -> String {
    let msg = serde_json::Value::String(message.to_string()).to_string();
    format!("{{\"ok\":false,\"code\":\"{code}\",\"message\":{msg}}}")
}

// ---------------------------------------------------------------------------
// JNI plumbing (safe `jni` API only)
// ---------------------------------------------------------------------------

pub(crate) fn rust_str(env: &mut JNIEnv<'_>, s: &JString<'_>) -> String {
    env.get_string(s)
        .map(|j| j.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub(crate) fn java_str<'x, 'a>(env: &'x JNIEnv<'a>, s: &str) -> JString<'a> {
    // Allocation failure here means the JVM is already OOM-dying; the expect
    // converts it into a bridge panic envelope via `guarded`'s caller path.
    // (This runs outside `guarded`, so keep the input small — envelopes only.)
    env.new_string(s)
        .unwrap_or_else(|_| env.new_string("").expect("empty JString must allocate"))
}

/// Run `f` panic-guarded, converting the result into a Java string envelope.
/// A Rust panic becomes `BRIDGE_PANIC` instead of unwinding into the JVM.
/// Takes `env` by value: `JString` is a raw-pointer wrapper (its lifetime is
/// the JNI frame's, not a borrow of `env`), so moving `env` is sound.
pub(crate) fn guarded<'a>(mut env: JNIEnv<'a>, f: impl FnOnce(&mut JNIEnv<'a>) -> String) -> JString<'a> {
    let out =
        std::panic::catch_unwind(AssertUnwindSafe(|| f(&mut env))).unwrap_or_else(|payload| {
            // Extract the panic message (String / &str / unknown) so the envelope
            // — and logcat — name the culprit instead of a bare "panicked".
            let msg = payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "unknown panic payload".to_string());
            let location = std::panic::Location::caller();
            // Full backtrace goes to logcat only (too long for a toast); the
            // envelope carries the message.
            let bt = std::backtrace::Backtrace::capture();
            tracing::error!("bridge panic at {location}: {msg}\n{bt}");
            let short = msg.chars().take(300).collect::<String>();
            err("BRIDGE_PANIC", format!("native bridge panicked: {short}"))
        });
    java_str(&env, &out)
}

// ---------------------------------------------------------------------------
// §6.1 implemented surface
// ---------------------------------------------------------------------------

/// `bridgeVersion() -> int`. Called before anything else.
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn Java_com_spotifydx_app_CoreBridge_bridgeVersion(
    _env: JNIEnv,
    _cls: JClass,
) -> jint {
    BRIDGE_VERSION
}

/// `initCore(filesDir, cacheDir) -> envelope`. Pins base directories and
/// kicks off bootstrap (adblock engine, auth restore) on the bridge runtime.
///
/// Returns immediately: directory pinning is synchronous (fast), while the
/// network/filesystem bootstrap previously `block_on`'d here — stalling the
/// calling thread (usually the main thread: ANR/startup jank). Kotlin learns
/// the outcome through the normal channels: the `session` event plus
/// `sessionStatus`, whose envelope carries `restoring` until the worker
/// lands, so the settled-gate never mistakes "not yet restored" for
/// "signed out". Must be called once from `Application.onCreate` (core
/// outlives the UI); re-entry returns at once.
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn Java_com_spotifydx_app_CoreBridge_initCore<'a>(
    env: JNIEnv<'a>,
    _cls: JClass<'a>,
    files_dir: JString<'a>,
    cache_dir: JString<'a>,
) -> JString<'a> {
    guarded(env, |env| {
        // Route Rust logs to logcat (see Cargo.toml android deps). stdout
        // goes nowhere on-device; without this all core diagnostics —
        // resolve outcomes, 401s, adblock stats — are invisible.
        android_logger::init_once(
            android_logger::Config::default()
                .with_tag("SpotifyDxNative")
                .with_max_level(log::LevelFilter::Info),
        );
        let files = rust_str(&mut *env, &files_dir);
        let cache = rust_str(&mut *env, &cache_dir);
        if files.is_empty() || cache.is_empty() {
            return err("INVALID_ARGS", "filesDir/cacheDir must be non-empty");
        }
        util::set_dir_overrides(files.into(), cache.into());
        updater::set_bridge_files_dir(util::data_dir());
        // Once-only bootstrap (activity recreations re-enter here): the
        // worker owns adblock/auth init exactly once.
        static BOOTSTRAP_STARTED: std::sync::atomic::AtomicBool =
            std::sync::atomic::AtomicBool::new(false);
        if BOOTSTRAP_STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return ok_data("{\"bridge\":1,\"session_restored\":false,\"started\":true}");
        }
        set_bootstrap_restoring(true);
        rt().spawn(async {
            // Panic-proof the flag: if this task ever dies early, the
            // restoring flag must still clear — otherwise every data call
            // parks the full 15s and the gate never settles (stuck boot).
            struct ClearOnDrop;
            impl Drop for ClearOnDrop {
                fn drop(&mut self) {
                    set_bootstrap_restoring(false);
                }
            }
            let _guard = ClearOnDrop;
            // Auth and adblock are independent: restore the session mirror
            // first (data calls unblock on it) while the engine builds.
            let (auth_res, adblock_res) = tokio::join!(
                async {
                    let restored = auth::init().await;
                    // Seed the mirror from the persisted token WITHOUT authenticating:
                    // clock-validity is unproven until `currentUser` verifies it or the
                    // login page captures (same rule as the dioxus gate — a stale stored
                    // token must never skip login).
                    if let Some((token, expires)) = auth::token_store::load() {
                        *session().lock().unwrap_or_else(|e| e.into_inner()) = BridgeSession {
                            authenticated: false,
                            access_token: Some(token.clone()),
                            expires_at_ms: expires,
                            user_json: None,
                        };
                        // Parked for fetchers too: a clock-valid stored token is usable
                        // until proven otherwise (same rule as the mirror seeding above).
                        crate::spotify::session::set_headless_token(token, expires);
                    }
                    restored
                },
                async {
                    if let Err(e) = adblock::init().await {
                        tracing::warn!(
                            "bridge: adblock init failed ({e:#}); continuing unfiltered"
                        );
                    }
                    // Background list refresh on the long-lived bridge runtime (init()
                    // no longer spawns it — see adguard_api::kick_refresh).
                    crate::adblock::adguard_api::kick_refresh();
                }
            );
            let _ = adblock_res;
            if auth_res {
                push_event("session", "{\"restored\":true}");
            }
            tracing::info!(
                "bridge: bootstrap done (auth_restored={auth_res}, token_seeded={})",
                session()
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .access_token
                    .is_some()
            );
            // Wake the Kotlin mirror either way: success lands via the event
            // above; the settled-gate must also release on a clean miss.
            push_event("session", "{\"bootstrap\":true}");
        });
        ok_data("{\"bridge\":1,\"session_restored\":false,\"started\":true}")
    })
}

/// `getSettings() -> envelope<Settings>`.
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn Java_com_spotifydx_app_CoreBridge_getSettings<'a>(
    env: JNIEnv<'a>,
    _cls: JClass<'a>,
) -> JString<'a> {
    guarded(env, |_env| {
        // Mirror into the runtime-free provider slot (NOT the SETTINGS
        // signal — signal access panics on bridge threads; see settings.rs).
        let loaded = settings::Settings::load();
        settings::sync_stream_credentials(&loaded);
        match serde_json::to_string(&loaded) {
            Ok(json) => ok_data(&json),
            Err(e) => err("IO", e),
        }
    })
}

/// `setSettings(json) -> envelope<Settings>` (normalizes + persists).
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn Java_com_spotifydx_app_CoreBridge_setSettings<'a>(
    env: JNIEnv<'a>,
    _cls: JClass<'a>,
    json: JString<'a>,
) -> JString<'a> {
    guarded(env, |env| {
        let raw = rust_str(&mut *env, &json);
        let mut parsed: settings::Settings = match serde_json::from_str(&raw) {
            Ok(p) => p,
            Err(e) => return err("INVALID_ARGS", format!("bad settings JSON: {e}")),
        };
        parsed.normalize();
        // Write-through to the runtime-free mirror (see getSettings).
        settings::sync_stream_credentials(&parsed);
        match parsed.save() {
            Ok(()) => match serde_json::to_string(&parsed) {
                Ok(j) => ok_data(&j),
                Err(e) => err("IO", e),
            },
            Err(e) => err("IO", e),
        }
    })
}

/// `getProfile() -> envelope<UserProfile>`.
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn Java_com_spotifydx_app_CoreBridge_getProfile<'a>(
    env: JNIEnv<'a>,
    _cls: JClass<'a>,
) -> JString<'a> {
    guarded(env, |_env| match serde_json::to_string(&profile::load()) {
        Ok(json) => ok_data(&json),
        Err(e) => err("IO", e),
    })
}

/// `setProfileName(name) -> envelope<UserProfile>`.
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn Java_com_spotifydx_app_CoreBridge_setProfileName<'a>(
    env: JNIEnv<'a>,
    _cls: JClass<'a>,
    name: JString<'a>,
) -> JString<'a> {
    guarded(env, |env| {
        let name = rust_str(&mut *env, &name);
        let mut p = profile::load();
        p.username = name;
        match profile::persist_to(&p, &profile::path()) {
            Ok(()) => serde_json::to_string(&p)
                .map(|j| ok_data(&j))
                .unwrap_or_else(|e| err("IO", e)),
            Err(e) => err("IO", e),
        }
    })
}

/// `setAvatar(bytes, mime) -> envelope<UserProfile>`. Same magic-byte + size
/// validation as the desktop settings screen.
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn Java_com_spotifydx_app_CoreBridge_setAvatar<'a>(
    env: JNIEnv<'a>,
    _cls: JClass<'a>,
    bytes: JByteArray<'a>,
    mime: JString<'a>,
) -> JString<'a> {
    guarded(env, |env| {
        let bytes = env.convert_byte_array(&bytes).unwrap_or_default();
        let mime = rust_str(&mut *env, &mime);
        let mut p = profile::load();
        if let Err(e) = profile::set_avatar(&mut p, Some(mime), &bytes) {
            return err("INVALID_ARGS", e);
        }
        match profile::persist_to(&p, &profile::path()) {
            Ok(()) => serde_json::to_string(&p)
                .map(|j| ok_data(&j))
                .unwrap_or_else(|e| err("IO", e)),
            Err(e) => err("IO", e),
        }
    })
}

/// `clearAvatar() -> envelope<UserProfile>`.
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn Java_com_spotifydx_app_CoreBridge_clearAvatar<'a>(
    env: JNIEnv<'a>,
    _cls: JClass<'a>,
) -> JString<'a> {
    guarded(env, |_env| {
        let mut p = profile::load();
        profile::clear_avatar(&mut p);
        match profile::persist_to(&p, &profile::path()) {
            Ok(()) => serde_json::to_string(&p)
                .map(|j| ok_data(&j))
                .unwrap_or_else(|e| err("IO", e)),
            Err(e) => err("IO", e),
        }
    })
}

/// `shouldBlockUrl(url) -> boolean`. Cheap-sync; fail-open on any error.
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn Java_com_spotifydx_app_CoreBridge_shouldBlockUrl<'a>(
    mut env: JNIEnv<'a>,
    _cls: JClass<'a>,
    url: JString<'a>,
) -> jboolean {
    let out = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let url = rust_str(&mut env, &url);
        adblock::should_block(&url) as jboolean
    }));
    out.unwrap_or(0)
}

/// `filterStats() -> envelope<AdblockStats>`.
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn Java_com_spotifydx_app_CoreBridge_filterStats<'a>(
    env: JNIEnv<'a>,
    _cls: JClass<'a>,
) -> JString<'a> {
    guarded(env, |_env| {
        match serde_json::to_string(&adblock::stats_snapshot()) {
            Ok(json) => ok_data(&json),
            Err(e) => err("IO", e),
        }
    })
}

/// `refreshFilters() -> envelope`. Trigger-only (list refresh stays
/// background-only, never blocking); Kotlin re-reads `filterStats` later.
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn Java_com_spotifydx_app_CoreBridge_refreshFilters<'a>(
    env: JNIEnv<'a>,
    _cls: JClass<'a>,
) -> JString<'a> {
    guarded(env, |_env| {
        rt().spawn(adblock::init());
        ok_str("refresh-started")
    })
}

/// `checkForUpdates() -> envelope<{current,latest,update_available}>`.
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn Java_com_spotifydx_app_CoreBridge_checkForUpdates<'a>(
    env: JNIEnv<'a>,
    _cls: JClass<'a>,
) -> JString<'a> {
    guarded(env, |_env| {
        let outcome: Result<String, String> = rt().block_on(async {
            let latest = updater::latest_release().await?;
            let available = updater::is_newer(&latest.version, updater::CURRENT_VERSION);
            *last_check().lock().unwrap_or_else(|e| e.into_inner()) = Some(latest.clone());
            let status = if available {
                format!("Update available: v{}", latest.version)
            } else {
                format!("Up to date (v{})", updater::CURRENT_VERSION)
            };
            set_update_status(status.clone());
            push_event(
                "update",
                &format!(
                    "{{\"check\":\"finished\",\"available\":{available},\"latest\":\"{}\"}}",
                    latest.version
                ),
            );
            Ok(serde_json::json!({
                "current": updater::CURRENT_VERSION,
                "latest": latest.version,
                "update_available": available,
            })
            .to_string())
        });
        match outcome {
            Ok(json) => ok_data(&json),
            Err(e) => {
                set_update_status(format!("Update check failed: {e}"));
                err("NET", e)
            }
        }
    })
}

/// `downloadUpdate() -> envelope<ReadyUpdate>`. Streams + hash-verifies into
/// the staged APK from the last `checkForUpdates`.
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn Java_com_spotifydx_app_CoreBridge_downloadUpdate<'a>(
    env: JNIEnv<'a>,
    _cls: JClass<'a>,
) -> JString<'a> {
    guarded(env, |_env| {
        let info = last_check()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let Some(info) = info else {
            return err(
                "INVALID_ARGS",
                "no checked release — call checkForUpdates first",
            );
        };
        let outcome = rt().block_on(updater::fetch_update(&info));
        match outcome {
            Ok(ready) => {
                set_update_status(format!(
                    "Update ready: v{} — apply to install",
                    ready.version
                ));
                push_event(
                    "update",
                    &format!("{{\"staged\":true,\"version\":\"{}\"}}", ready.version),
                );
                ok_data(&format!("{{\"version\":\"{}\"}}", ready.version))
            }
            Err(e) => {
                set_update_status(format!("Update download failed: {e}"));
                err("NET", e)
            }
        }
    })
}

/// `applyUpdate(activity) -> envelope`. Hands the staged APK to the system
/// package installer via the existing `SpotifyDxUpdater` + file provider.
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn Java_com_spotifydx_app_CoreBridge_applyUpdate<'a>(
    env: JNIEnv<'a>,
    _cls: JClass<'a>,
    activity: JObject<'a>,
) -> JString<'a> {
    guarded(env, |env| {
        match updater::request_android_install_with(env, &activity) {
            Ok(()) => {
                set_update_status("Installing update…".into());
                ok_str("installer-launched")
            }
            Err(e) => err("IO", e),
        }
    })
}

/// `updateStatus() -> envelope<{status}>`. Bridge-side mirror of the dioxus
/// `UPDATE_STATUS` global (unreadable headless — no runtime).
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn Java_com_spotifydx_app_CoreBridge_updateStatus<'a>(
    env: JNIEnv<'a>,
    _cls: JClass<'a>,
) -> JString<'a> {
    guarded(env, |_env| {
        let s = update_status()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        ok_data(&serde_json::Value::String(s).to_string())
    })
}

/// `sessionStatus() -> envelope<{authenticated,expires_at_ms,has_token,user}>`.
/// Same boolean semantics as the dioxus `AUTH_STATE` gate.
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn Java_com_spotifydx_app_CoreBridge_sessionStatus<'a>(
    env: JNIEnv<'a>,
    _cls: JClass<'a>,
) -> JString<'a> {
    guarded(env, |_env| {
        let s = session().lock().unwrap_or_else(|e| e.into_inner()).clone();
        ok_data(&serde_json::json!({
            "authenticated": s.authenticated,
            "has_token": s.access_token.is_some(),
            "expires_at_ms": s.expires_at_ms,
            "restoring": BOOTSTRAP_RESTORING.load(std::sync::atomic::Ordering::SeqCst),
            "user": s.user_json.map(|u| serde_json::from_str::<serde_json::Value>(&u).unwrap_or(serde_json::Value::Null)).unwrap_or(serde_json::Value::Null),
        }).to_string())
    })
}

/// `notifySession(json) -> envelope`. Kotlin reports a captured web-player
/// session (`{access_token, expires_at_ms, user?}`); the bridge persists the
/// token (keychain/file, same as `on_session_captured`) and flips the mirror.
/// Anonymous captures are rejected like the dioxus gate rejects them.
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn Java_com_spotifydx_app_CoreBridge_notifySession<'a>(
    env: JNIEnv<'a>,
    _cls: JClass<'a>,
    json: JString<'a>,
) -> JString<'a> {
    guarded(env, |env| {
        let raw = rust_str(&mut *env, &json);
        let v: serde_json::Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => return err("INVALID_ARGS", format!("bad session JSON: {e}")),
        };
        let token = v
            .get("access_token")
            .and_then(|t| t.as_str())
            .unwrap_or_default();
        if token.is_empty() {
            return err("INVALID_ARGS", "session has no access_token");
        }
        if v.get("is_anonymous")
            .and_then(|a| a.as_bool())
            .unwrap_or(false)
        {
            return err("INVALID_ARGS", "anonymous sessions do not authenticate");
        }
        let expires_at_ms = v.get("expires_at_ms").and_then(|e| e.as_u64()).unwrap_or(0);
        auth::token_store::save(token, expires_at_ms);
        crate::spotify::session::set_headless_token(token.to_string(), expires_at_ms);
        let user_json = v.get("user").map(|u| u.to_string());
        // Premium mirror (see currentUser): capture carries the user object.
        let premium = v
            .get("user")
            .and_then(|u| u.get("product"))
            .and_then(|p| p.as_str())
            == Some("premium");
        crate::settings::set_session_premium(premium);
        *session().lock().unwrap_or_else(|e| e.into_inner()) = BridgeSession {
            authenticated: true,
            access_token: Some(token.to_string()),
            expires_at_ms,
            user_json: user_json.clone(),
        };
        push_event("session", "{\"authenticated\":true}");
        ok_str("session-stored")
    })
}

/// `logout() -> envelope`. Clears the credential store + fallback and resets
/// the mirror (tearing down cookie pages is Kotlin's job — `LoginWebView`
/// owns the pages in this app, per §8).
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn Java_com_spotifydx_app_CoreBridge_logout<'a>(
    env: JNIEnv<'a>,
    _cls: JClass<'a>,
) -> JString<'a> {
    guarded(env, |_env| {
        // Single choke point: clears the credential store (+ native page
        // teardown where present; headless-safe by construction) while the
        // lines below reset the bridge-owned mirror and provider slot.
        auth::logout();
        crate::spotify::session::clear_headless_token();
        crate::settings::set_session_premium(false);
        *session().lock().unwrap_or_else(|e| e.into_inner()) = BridgeSession::default();
        push_event("session", "{\"authenticated\":false}");
        ok_str("logged-out")
    })
}

/// `pollEvents() -> envelope<[{kind,payload}]>`. Drains the core→Kotlin event
/// queue (§6.3 via polled future — explicitly allowed by §6). Kotlin long-
/// polls this from a background coroutine and dispatches to its event bus.
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn Java_com_spotifydx_app_CoreBridge_pollEvents<'a>(
    env: JNIEnv<'a>,
    _cls: JClass<'a>,
) -> JString<'a> {
    guarded(env, |_env| {
        let mut q = events().lock().unwrap_or_else(|e| e.into_inner());
        let drained: Vec<String> = q.drain(..).collect();
        ok_data(&format!("[{}]", drained.join(",")))
    })
}


// ---------------------------------------------------------------------------
/// Mirror freshness gate shared by every data call: the bridge upholds the
/// same 60s skew as `ensure_token`. Stale/missing → `NEEDS_PAGE` (Kotlin
/// revives the session page through `SessionRefresher` and retries) instead
/// of a doomed network call.
pub(crate) fn need_fresh_token() -> Result<String, String> {
    // Cold-start ordering: initCore returns before the bootstrap worker
    // seeds the mirror, but shell-first screens fire data calls within ms.
    // Without this wait they all fail NEEDS_PAGE against an empty-but-
    // healthy mirror (stuck "signing you back in" + dead retry). Park
    // boundedly for the worker — data calls run on Binder/IO threads,
    // never the main thread, so this restores the old ordering with no
    // ANR risk. A hung worker still degrades to NEEDS_PAGE after 15s.
    await_bootstrap_blocking();
    // Read only the two compared fields under the lock — cloning the whole
    // session (including the user_json blob) on every data call was pure
    // waste at this call volume.
    let guard = session().lock().unwrap_or_else(|e| e.into_inner());
    match guard.access_token.clone() {
        Some(t) if guard.expires_at_ms > now_ms() + TOKEN_SKEW_MS => Ok(t),
        _ => Err(err(
            "NEEDS_PAGE",
            "token stale or missing — revive the session page",
        )),
    }
}

/// Bounded park for the initCore bootstrap worker (see `need_fresh_token`).
/// Fast path (worker done) costs one atomic load.
fn await_bootstrap_blocking() {
    if !BOOTSTRAP_RESTORING.load(std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while BOOTSTRAP_RESTORING.load(std::sync::atomic::Ordering::SeqCst) {
        if std::time::Instant::now() >= deadline {
            tracing::warn!("bridge: bootstrap wait timed out; proceeding unseeded");
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// Map a data-layer failure to its envelope. `NoBridgeSession` (headless
/// guard tripped on a race) heals via page revive; `Auth` (e.g. the 401
/// revoked-session arm, which now runs headless-safe through `auth::logout`)
/// is the first-class expiry transition; rate limits stay typed for the
/// banner + timed auto-retry contract.
pub(crate) fn map_data_err(e: crate::app_error::AppError) -> String {
    // Revoked sessions expire the mirror + provider slot HERE (not just the
    // store, which `auth::logout` already cleared): otherwise the next data
    // call would reuse the dead token and 401-loop. Kotlin gates on the code
    // and finishes the transition with a store+mirror logout of its own.
    if matches!(e, crate::app_error::AppError::Auth(_)) {
        session()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .authenticated = false;
        crate::spotify::session::clear_headless_token();
        push_event("session", "{\"authenticated\":false}");
    }
    match &e {
        crate::app_error::AppError::NoBridgeSession => {
            err("NEEDS_PAGE", "no session — revive the login page")
        }
        crate::app_error::AppError::Auth(_) => {
            err("SESSION_EXPIRED", "session rejected — please sign in again")
        }
        crate::app_error::AppError::RateLimited => err("RATE_LIMITED", e.to_string()),
        // Surfaced typed (not NET) so the Kotlin engine router can tell
        // "not Premium" apart from a transport failure and fall back to the
        // open engine / show the upsell instead of a generic error.
        crate::app_error::AppError::PremiumRequired(_) => err("PREMIUM_REQUIRED", e.to_string()),
        _ => err("NET", e.to_string()),
    }
}

pub(crate) fn to_json<T: serde::Serialize>(v: &T) -> String {
    match serde_json::to_string(v) {
        Ok(j) => ok_data(&j),
        Err(e) => err("IO", e),
    }
}

// §6.1 session surface, Phase 2 (real)
// ---------------------------------------------------------------------------

/// `beginLogin(arg) -> envelope<{authenticated,login_url?}>`. The gate asks
/// the core to begin login; an already-authenticated mirror short-circuits
/// (no page opened), otherwise Kotlin hosts the returned URL fullscreen.
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn Java_com_spotifydx_app_CoreBridge_beginLogin<'a>(
    env: JNIEnv<'a>,
    _cls: JClass<'a>,
    _arg: JString<'a>,
) -> JString<'a> {
    guarded(env, |_env| {
        let authenticated = session()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .authenticated;
        if authenticated {
            ok_data("{\"authenticated\":true}")
        } else {
            ok_data(&format!(
                "{{\"authenticated\":false,\"login_url\":\"{SIGN_IN_URL}\"}}"
            ))
        }
    })
}

/// `refreshToken(arg) -> envelope<{fresh}>`. Documented inversion (§8): the
/// login/session page lives in Kotlin, so the core cannot mint through it —
/// it only judges the mirror. Fresh → `{fresh:true}` (no token value ever
/// crosses; Kotlin never needs the raw token). Stale/missing → `NEEDS_PAGE`,
/// and Kotlin revives the page, captures, and retries (same bounded,
/// fan-out semantics as the native refresh, driven from the page owner).
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn Java_com_spotifydx_app_CoreBridge_refreshToken<'a>(
    env: JNIEnv<'a>,
    _cls: JClass<'a>,
    _arg: JString<'a>,
) -> JString<'a> {
    guarded(env, |_env| {
        let s = session().lock().unwrap_or_else(|e| e.into_inner()).clone();
        match s.access_token {
            Some(_) if s.expires_at_ms > now_ms() + TOKEN_SKEW_MS => ok_data("{\"fresh\":true}"),
            _ => err(
                "NEEDS_PAGE",
                "token stale or missing — revive the session page",
            ),
        }
    })
}

/// `currentUser(arg) -> envelope<{id,display_name,product,avatar_url}>`.
/// Single low-volume `/v1/me` read (the one public-REST exception, same as
/// the native login path) over the mirrored token. Success also flips the
/// mirror to authenticated — this is the silent-restore verifier. A 401 maps
/// to `SESSION_EXPIRED` (Kotlin clears + gates); rate limits stay typed.
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn Java_com_spotifydx_app_CoreBridge_currentUser<'a>(
    env: JNIEnv<'a>,
    _cls: JClass<'a>,
    _arg: JString<'a>,
) -> JString<'a> {
    guarded(env, |_env| {
        let token = {
            let guard = session().lock().unwrap_or_else(|e| e.into_inner());
            match guard.access_token.clone() {
                Some(t) if guard.expires_at_ms > now_ms() + TOKEN_SKEW_MS => t,
                _ => {
                    return err(
                        "NEEDS_PAGE",
                        "token stale or missing — revive the session page",
                    )
                }
            }
        };
        let outcome: Result<models::UserProfile, crate::app_error::AppError> =
            rt().block_on(async {
                let resp =
                    client::filtered_get_auth("https://api.spotify.com/v1/me", &token).await?;
                // Typed expiry: a 401 here is a rejected token, not a generic
                // network failure. (The old `msg.contains("401")` substring
                // match worked only because reqwest renders the status into the
                // message — fragile next to the typed `SessionExpired` the rest
                // of the auth flow uses.)
                if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
                    return Err(crate::app_error::AppError::SessionExpired);
                }
                let profile: models::UserProfile = resp
                    .error_for_status()
                    .map_err(crate::app_error::AppError::from)?
                    .json()
                    .await
                    .map_err(crate::app_error::AppError::from)?;
                Ok(profile)
            });
        match outcome {
            Ok(p) => {
                let user_json = serde_json::json!({
                    "id": p.id,
                    "display_name": p.display_name,
                    "product": p.product.clone(),
                    "avatar_url": p.images.first().map(|i| i.url.clone()).unwrap_or_default(),
                })
                .to_string();
                {
                    let mut s = session().lock().unwrap_or_else(|e| e.into_inner());
                    s.authenticated = true;
                    s.user_json = Some(user_json.clone());
                }
                // Premium mirror for the runtime-free SDK gate (never touches
                // AUTH_STATE — signal access panics on bridge threads).
                crate::settings::set_session_premium(p.product.as_deref() == Some("premium"));
                push_event("session", "{\"authenticated\":true}");
                ok_data(&user_json)
            }
            Err(e) => {
                // A rejected token is a first-class expiry transition: drop
                // the authenticated flag so the gate returns (cookies usually
                // re-authenticate silently there).
                let expired = matches!(e, crate::app_error::AppError::SessionExpired);
                if expired {
                    session()
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .authenticated = false;
                    push_event("session", "{\"authenticated\":false}");
                    err("SESSION_EXPIRED", "session rejected — please sign in again")
                } else if matches!(e, crate::app_error::AppError::RateLimited) {
                    err("RATE_LIMITED", e.to_string())
                } else {
                    err("NET", e.to_string())
                }
            }
        }
    })
}
