//! JNI data surface (§6.1, Phase 3 + resolveStream + PHASE_2_PLUS stubs).
//!
//! Moved verbatim out of `bridge.rs` (file split, no logic touched):
//! Spotify data fetchers, artwork, stream resolution, and the unwired-call
//! stubs. Shared plumbing (`guarded`, envelopes, token gate, error map)
//! stays in the parent module.

use jni::objects::{JClass, JString};
use jni::JNIEnv;

use super::{err, guarded, map_data_err, need_fresh_token, ok_data, rt, rust_str, to_json};

/// Error code for §6.1 calls whose core phase has not landed yet. Kotlin maps
/// it to its own error hierarchy like any other typed core error.
const PHASE_CODE: &str = "PHASE_2_PLUS";
const PHASE_MSG: &str = "wired in a later migration phase (see docs/KOTLIN_MIGRATION.md)";

// ---------------------------------------------------------------------------
// §6.1 data surface, Phase 3 (real)
// ---------------------------------------------------------------------------

/// `getHome(arg) -> envelope<HomeData>`. Playlists + liked tracks fanned out
/// concurrently in core; partial legs default (never a hard failure).
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn Java_com_spotifydx_app_CoreBridge_getHome<'a>(
    env: JNIEnv<'a>,
    _cls: JClass<'a>,
    _arg: JString<'a>,
) -> JString<'a> {
    guarded(env, |_env| {
        if let Err(e) = need_fresh_token() {
            return e;
        }
        match rt().block_on(crate::spotify::api::get_home()) {
            Ok(home) => to_json(&home),
            Err(e) => map_data_err(e),
        }
    })
}

/// `search(arg) -> envelope<SearchResults>`. `{query, limit?}`; multi-type
/// GQL search, same operation the desktop Search page uses.
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn Java_com_spotifydx_app_CoreBridge_search<'a>(
    env: JNIEnv<'a>,
    _cls: JClass<'a>,
    arg: JString<'a>,
) -> JString<'a> {
    guarded(env, |env| {
        if let Err(e) = need_fresh_token() {
            return e;
        }
        let raw = rust_str(&mut *env, &arg);
        let v: serde_json::Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => return err("INVALID_ARGS", format!("bad arg JSON: {e}")),
        };
        let q = v.get("query").and_then(|q| q.as_str()).unwrap_or_default();
        if q.is_empty() {
            return err("INVALID_ARGS", "arg.query must be non-empty");
        }
        let limit = v
            .get("limit")
            .and_then(|l| l.as_u64())
            .unwrap_or(20)
            .min(50)
            .max(1) as u32;
        match rt().block_on(crate::spotify::api::search(
            q,
            &["track", "album", "artist"],
            limit,
        )) {
            Ok(results) => to_json(&results),
            Err(e) => map_data_err(e),
        }
    })
}

/// `getPlaylist(arg) -> envelope<Playlist>`. `{id}`; detail + tracks.
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn Java_com_spotifydx_app_CoreBridge_getPlaylist<'a>(
    env: JNIEnv<'a>,
    _cls: JClass<'a>,
    arg: JString<'a>,
) -> JString<'a> {
    guarded(env, |env| {
        if let Err(e) = need_fresh_token() {
            return e;
        }
        let raw = rust_str(&mut *env, &arg);
        let id = match crate::util::arg_id(&raw) {
            Ok(id) => id,
            Err(e) => return err("INVALID_ARGS", e),
        };
        match rt().block_on(crate::spotify::gql::gql_playlist(&id)) {
            Ok(playlist) => to_json(&playlist),
            Err(e) => map_data_err(e),
        }
    })
}

/// `getAlbum(arg) -> envelope<{album, tracks}>`. `{id}`; detail + track
/// listing merged (the GQL album shape carries metadata; tracks come from
/// the companion call, same as the desktop detail page).
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn Java_com_spotifydx_app_CoreBridge_getAlbum<'a>(
    env: JNIEnv<'a>,
    _cls: JClass<'a>,
    arg: JString<'a>,
) -> JString<'a> {
    guarded(env, |env| {
        if let Err(e) = need_fresh_token() {
            return e;
        }
        let raw = rust_str(&mut *env, &arg);
        let id = match crate::util::arg_id(&raw) {
            Ok(id) => id,
            Err(e) => return err("INVALID_ARGS", e),
        };
        let outcome = rt().block_on(async {
            let album = crate::spotify::api::get_album(&id).await?;
            // Tracks are best-effort: a failing leg yields the header with an
            // empty listing, never a crash (partial-failure tolerance).
            let tracks = crate::spotify::api::get_album_tracks(&id)
                .await
                .unwrap_or_default();
            Ok::<_, crate::app_error::AppError>((album, tracks))
        });
        match outcome {
            Ok((album, tracks)) => to_json(&serde_json::json!({
                "album": album,
                "tracks": tracks,
            })),
            Err(e) => map_data_err(e),
        }
    })
}

/// `getArtistPage(arg) -> envelope<ArtistPage>`. `{id}`; hero + discography
/// + popular tracks + related in minimal round trips.
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn Java_com_spotifydx_app_CoreBridge_getArtistPage<'a>(
    env: JNIEnv<'a>,
    _cls: JClass<'a>,
    arg: JString<'a>,
) -> JString<'a> {
    guarded(env, |env| {
        if let Err(e) = need_fresh_token() {
            return e;
        }
        let raw = rust_str(&mut *env, &arg);
        let id = match crate::util::arg_id(&raw) {
            Ok(id) => id,
            Err(e) => return err("INVALID_ARGS", e),
        };
        match rt().block_on(crate::spotify::api::get_artist_page(&id)) {
            Ok(page) => to_json(&page),
            Err(e) => map_data_err(e),
        }
    })
}

/// `getLikedTracks(arg) -> envelope<Paged<SavedTrack>>`. `{limit, offset}`;
/// fixed page sizes, `null`-track skipping for region-blocked items happens
/// Kotlin-side (the envelope preserves them).
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn Java_com_spotifydx_app_CoreBridge_getLikedTracks<'a>(
    env: JNIEnv<'a>,
    _cls: JClass<'a>,
    arg: JString<'a>,
) -> JString<'a> {
    guarded(env, |env| {
        if let Err(e) = need_fresh_token() {
            return e;
        }
        let raw = rust_str(&mut *env, &arg);
        let (limit, offset) = match crate::util::arg_page(&raw) {
            Ok(p) => p,
            Err(e) => return err("INVALID_ARGS", e),
        };
        match rt().block_on(crate::spotify::api::get_user_saved_tracks(limit, offset)) {
            Ok(paged) => to_json(&paged),
            Err(e) => map_data_err(e),
        }
    })
}

/// `getLibrary(arg) -> envelope`. `{kind: playlists|albums|liked, limit,
/// offset}`; one bridge call per collection leg (Kotlin fans the trio out
/// concurrently, any-leg failure surfaces).
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn Java_com_spotifydx_app_CoreBridge_getLibrary<'a>(
    env: JNIEnv<'a>,
    _cls: JClass<'a>,
    arg: JString<'a>,
) -> JString<'a> {
    guarded(env, |env| {
        if let Err(e) = need_fresh_token() {
            return e;
        }
        let raw = rust_str(&mut *env, &arg);
        let v: serde_json::Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => return err("INVALID_ARGS", format!("bad arg JSON: {e}")),
        };
        let kind = v.get("kind").and_then(|k| k.as_str()).unwrap_or_default();
        if !matches!(kind, "playlists" | "albums" | "liked") {
            return err("INVALID_ARGS", "arg.kind must be playlists|albums|liked");
        }
        let (limit, offset) = match crate::util::arg_page(&raw) {
            Ok(p) => p,
            Err(e) => return err("INVALID_ARGS", e),
        };
        let outcome = rt().block_on(async {
            match kind {
                "playlists" => crate::spotify::gql::gql_user_playlists(limit, offset)
                    .await
                    .map(|items| to_json(&items)),
                "albums" => crate::spotify::gql::gql_user_albums(limit, offset)
                    .await
                    .map(|items| to_json(&items)),
                _ => crate::spotify::api::get_user_saved_tracks(limit, offset)
                    .await
                    .map(|paged| to_json(&paged)),
            }
        });
        match outcome {
            Ok(envelope) => envelope,
            Err(e) => map_data_err(e),
        }
    })
}

/// `fetchArtwork(arg) -> envelope<string>`. `{url}`; base64 bytes through the
/// core's hash-keyed disk cache + ad-filter gate (zero JNI traffic on repeat
/// hits is Kotlin's memory layer; the core cache is the disk tier). Empty
/// URLs fail fast with `INVALID_ARGS`.
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn Java_com_spotifydx_app_CoreBridge_fetchArtwork<'a>(
    env: JNIEnv<'a>,
    _cls: JClass<'a>,
    arg: JString<'a>,
) -> JString<'a> {
    guarded(env, |env| {
        let raw = rust_str(&mut *env, &arg);
        let v: serde_json::Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => return err("INVALID_ARGS", format!("bad arg JSON: {e}")),
        };
        let url = v.get("url").and_then(|u| u.as_str()).unwrap_or_default();
        if url.is_empty() {
            return err("INVALID_ARGS", "arg.url must be non-empty");
        }
        match rt().block_on(crate::media::images::load(url)) {
            Ok(bytes) => {
                use base64::Engine;
                ok_data(
                    &serde_json::Value::String(
                        base64::engine::general_purpose::STANDARD.encode(bytes.as_ref()),
                    )
                    .to_string(),
                )
            }
            Err(e) => err("NET", e.to_string()),
        }
    })
}

// ---------------------------------------------------------------------------
// §6.1 later-phase surface: exact signatures, PHASE_2_PLUS until wired
// ---------------------------------------------------------------------------

/// Generate a `PHASE_2_PLUS` stub with the exact §6.1 signature. All stubs
/// take an optional JSON arg (empty string when the call needs none) so the
/// Kotlin declaration never changes when the real implementation lands.
macro_rules! phase_stub {
    ($java:ident) => {
        #[allow(unsafe_code)]
        #[no_mangle]
        pub extern "C" fn $java<'a>(
            env: JNIEnv<'a>,
            _cls: JClass<'a>,
            _arg: JString<'a>,
        ) -> JString<'a> {
            guarded(env, |_| err(PHASE_CODE, PHASE_MSG))
        }
    };
}

/// `resolveStream(arg) -> envelope<{url,format,provider}>`. `{track}` as a
/// core `Track` object (rows/cards always pass the visible object —
/// play-with-metadata, zero network). Stream-URL cache first, ordered
/// provider failover behind it (sole active provider: YouTube-muxed,
/// §6.9). Deliberately NOT gated on a fresh Spotify token: resolution is
/// token-free (no Spotify API call), so an expired session must never
/// block playback — the previous gate turned every post-expiry tap into
/// an instant NEEDS_PAGE that the player couldn't heal (only screens
/// heal), forcing the user through a manual sign-in + retry loop before
/// any new song would play.
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn Java_com_spotifydx_app_CoreBridge_resolveStream<'a>(
    env: JNIEnv<'a>,
    _cls: JClass<'a>,
    arg: JString<'a>,
) -> JString<'a> {
    guarded(env, |env| {
        // No need_fresh_token gate (see doc above): token-free resolution.
        let raw = rust_str(&mut *env, &arg);
        let v: serde_json::Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => return err("INVALID_ARGS", format!("bad arg JSON: {e}")),
        };
        let track: crate::spotify::models::Track = match serde_json::from_value(
            v.get("track").cloned().unwrap_or(serde_json::Value::Null),
        ) {
            Ok(t) => t,
            Err(e) => return err("INVALID_ARGS", format!("arg.track is not a Track: {e}")),
        };
        // Advisory quality ceiling from the Android wifi/metered
        // preference; absent/unknown = unconstrained (legacy behavior).
        let hint = crate::streaming::provider::Quality::parse_hint(
            v.get("quality").and_then(|q| q.as_str()),
        );
        match rt().block_on(crate::streaming::resolver::resolve(&track, hint)) {
            Ok(Some(r)) => ok_data(
                &serde_json::json!({
                    "url": r.url,
                    "format": format!("{:?}", r.format).to_lowercase(),
                    "provider": r.provider,
                    "quality": format!("{:?}", r.quality).to_lowercase(),
                })
                .to_string(),
            ),
            Ok(None) => err("NOT_FOUND", "no playable source found"),
            Err(e) => err("NET", e),
        }
    })
}

phase_stub!(Java_com_spotifydx_app_CoreBridge_playTrack);
phase_stub!(Java_com_spotifydx_app_CoreBridge_playUri);
phase_stub!(Java_com_spotifydx_app_CoreBridge_pause);
phase_stub!(Java_com_spotifydx_app_CoreBridge_resume);
phase_stub!(Java_com_spotifydx_app_CoreBridge_next);
phase_stub!(Java_com_spotifydx_app_CoreBridge_prev);
phase_stub!(Java_com_spotifydx_app_CoreBridge_seek);
phase_stub!(Java_com_spotifydx_app_CoreBridge_setVolume);
phase_stub!(Java_com_spotifydx_app_CoreBridge_enqueue);
