//! JNI SDK-transport surface (Phase 5, §9.2) + lyrics fetch.
//!
//! Moved verbatim out of `bridge.rs` (file split, no logic touched):
//! the SDK document source, Connect transport commands, state parsing,
//! and the lyrics fetch. Shared plumbing stays in the parent module.

use jni::objects::{JClass, JString};
use jni::JNIEnv;

use super::{err, guarded, map_data_err, need_fresh_token, ok_data, ok_str, rt, rust_str};

// -- SDK path (Phase 5, §9.2 migration): Connect transport against the
// -- Kotlin-hosted SDK device. The device itself lives in a hidden platform
// -- WebView driven from Kotlin (`SdkWebViewDriver`); these calls are the
// -- "transport commands map to the Connect control calls the core already
// -- makes" half — thin arg parsing over `spotify::player_api`, whose
// -- `require_premium` gate stays intact (tier folded into AUTH_STATE by
// -- `currentUser` in the parent module). The Dioxus-era `playTrack/pause/...`
// -- stubs are a different (dead) surface living in `bridge_data` — untouched.
fn sdk_str_arg(v: &serde_json::Value, key: &str) -> Result<String, String> {
    v.get(key)
        .and_then(|s| s.as_str())
        .map(|s| s.to_owned())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| err("INVALID_ARGS", format!("arg.{key} missing or empty")))
}

fn sdk_json_arg(env: &mut JNIEnv<'_>, arg: &JString<'_>) -> Result<serde_json::Value, String> {
    let raw = rust_str(env, arg);
    serde_json::from_str(&raw).map_err(|e| err("INVALID_ARGS", format!("bad arg JSON: {e}")))
}

/// `sdkDocument("") -> envelope{data: JSON-encoded SDK_HTML}`. Single-sourced:
/// Kotlin must never duplicate the document (`player::playback_sdk::SDK_HTML`
/// is the only copy). No token needed — the document carries no credentials.
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn Java_com_spotifydx_app_CoreBridge_sdkDocument<'a>(
    env: JNIEnv<'a>,
    _cls: JClass<'a>,
    _arg: JString<'a>,
) -> JString<'a> {
    guarded(env, |_| {
        ok_data(
            &serde_json::Value::String(crate::player::playback_sdk::SDK_HTML.to_owned())
                .to_string(),
        )
    })
}

macro_rules! sdk_transport {
    ($java:ident, $op:expr) => {
        #[allow(unsafe_code)]
        #[no_mangle]
        pub extern "C" fn $java<'a>(
            env: JNIEnv<'a>,
            _cls: JClass<'a>,
            arg: JString<'a>,
        ) -> JString<'a> {
            guarded(env, |env| {
                if let Err(e) = need_fresh_token() {
                    return e;
                }
                let v = match sdk_json_arg(env, &arg) {
                    Ok(v) => v,
                    Err(e) => return e,
                };
                let device_id = match sdk_str_arg(&v, "device_id") {
                    Ok(d) => d,
                    Err(e) => return e,
                };
                // `$op(device_id, v)` builds the future inline so its borrows
                // stay concrete (a closure-returned future trips higher-ranked
                // lifetime errors against `block_on`).
                match rt().block_on($op(&device_id, &v)) {
                    Ok(()) => ok_str("ok"),
                    Err(e) => map_data_err(e),
                }
            })
        }
    };
}

async fn sdk_do_play(
    device: &str,
    v: &serde_json::Value,
) -> Result<(), crate::app_error::AppError> {
    let uri = sdk_str_arg(v, "uri").map_err(crate::app_error::AppError::Playback)?;
    let pos = v.get("position_ms").and_then(|n| n.as_u64());
    crate::spotify::player_api::play(device, &uri, pos).await
}

async fn sdk_do_pause(
    device: &str,
    _v: &serde_json::Value,
) -> Result<(), crate::app_error::AppError> {
    crate::spotify::player_api::pause(device).await
}

async fn sdk_do_skip(
    device: &str,
    v: &serde_json::Value,
) -> Result<(), crate::app_error::AppError> {
    let next = v.get("next").and_then(|b| b.as_bool()).unwrap_or(true);
    crate::spotify::player_api::skip(device, next).await
}

async fn sdk_do_seek(
    device: &str,
    v: &serde_json::Value,
) -> Result<(), crate::app_error::AppError> {
    let ms = v.get("position_ms").and_then(|n| n.as_u64()).unwrap_or(0);
    crate::spotify::player_api::seek(device, ms).await
}

async fn sdk_do_volume(
    device: &str,
    v: &serde_json::Value,
) -> Result<(), crate::app_error::AppError> {
    // Clamp contract (0–100): u64 clamps as before; negatives clamp to 0
    // instead of silently becoming the 80 default; non-numeric/missing
    // keeps the legacy 80.
    let pct = match v.get("volume") {
        Some(n) if n.as_u64().is_some() => n.as_u64().unwrap_or(80).min(100),
        Some(n) if n.as_i64().is_some() => n.as_i64().unwrap_or(80).max(0).min(100) as u64,
        _ => 80,
    } as u8;
    crate::spotify::player_api::set_volume(device, pct).await
}

sdk_transport!(Java_com_spotifydx_app_CoreBridge_sdkPlay, sdk_do_play);
sdk_transport!(Java_com_spotifydx_app_CoreBridge_sdkPause, sdk_do_pause);
sdk_transport!(Java_com_spotifydx_app_CoreBridge_sdkSkip, sdk_do_skip);
sdk_transport!(Java_com_spotifydx_app_CoreBridge_sdkSeek, sdk_do_seek);
sdk_transport!(Java_com_spotifydx_app_CoreBridge_sdkVolume, sdk_do_volume);

/// `sdkParseState(payload) -> envelope{data: SdkState JSON}`. Reuses the
/// tested core parser (`playback_sdk::parse_sdk_state`); unknown shapes
/// degrade to defaults, never errors.
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn Java_com_spotifydx_app_CoreBridge_sdkParseState<'a>(
    env: JNIEnv<'a>,
    _cls: JClass<'a>,
    arg: JString<'a>,
) -> JString<'a> {
    guarded(env, |env| {
        let v = match sdk_json_arg(env, &arg) {
            Ok(v) => v,
            Err(e) => return e,
        };
        let state = crate::player::playback_sdk::parse_sdk_state(&v);
        match serde_json::to_value(&state) {
            Ok(json) => ok_data(&json.to_string()),
            Err(e) => err("NET", format!("state encode failed: {e}")),
        }
    })
}

/// `fetchLyrics(arg) -> envelope<LyricsResult>`.
/// `{artist, title, album?, duration_ms?}`. Misses are normal (many tracks
/// have no lyrics) and return `found: false`, never an error.
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn Java_com_spotifydx_app_CoreBridge_fetchLyrics<'a>(
    env: JNIEnv<'a>,
    _cls: JClass<'a>,
    arg: JString<'a>,
) -> JString<'a> {
    guarded(env, |env| {
        let v = match sdk_json_arg(env, &arg) {
            Ok(v) => v,
            Err(e) => return e,
        };
        let str_field = |key: &str| {
            v.get(key)
                .and_then(|s| s.as_str())
                .unwrap_or_default()
                .to_string()
        };
        let artist = str_field("artist");
        let title = str_field("title");
        if artist.is_empty() || title.is_empty() {
            return err("INVALID_ARGS", "arg.artist and arg.title are required");
        }
        let album = str_field("album");
        let duration_ms = v.get("duration_ms").and_then(|n| n.as_u64()).unwrap_or(0);
        match rt().block_on(crate::streaming::lyrics::fetch_lyrics(
            &artist,
            &title,
            &album,
            duration_ms,
        )) {
            Ok(lyrics) => match serde_json::to_value(&lyrics) {
                Ok(json) => ok_data(&json.to_string()),
                Err(e) => err("NET", format!("lyrics encode failed: {e}")),
            },
            Err(e) => map_data_err(e),
        }
    })
}
