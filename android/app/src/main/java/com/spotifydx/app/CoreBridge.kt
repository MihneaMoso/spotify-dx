package com.spotifydx.app

/**
 * JNI declarations for the native core (`libspotify_dx.so`, `src/bridge.rs`).
 *
 * Every method maps 1:1 to a `Java_com_spotifydx_app_CoreBridge_<method>`
 * C-ABI export; the bridge-compat check (docs/KOTLIN_MIGRATION.md §13) fails
 * the build on any signature drift. All `String` returns are
 * `{"ok","code","message","data"}` envelopes (see [BridgeClient]); the two
 * cheap-sync calls ([bridgeVersion], [shouldBlockUrl]) are bare values.
 *
 * Threading (§12.1): blocking calls run on `Dispatchers.IO` via
 * [BridgeClient]; cheap-sync calls may run anywhere. Never call blocking
 * methods on the main thread.
 */
class CoreBridge private constructor() {
    companion object {
        const val EXPECTED_VERSION = 1

        init {
            System.loadLibrary("spotify_dx")
        }

        // -- Handshake + lifecycle ------------------------------------------------
        @JvmStatic external fun bridgeVersion(): Int
        @JvmStatic external fun initCore(filesDir: String, cacheDir: String): String

        // -- Settings / profile (§11) ---------------------------------------------
        @JvmStatic external fun getSettings(): String
        @JvmStatic external fun setSettings(json: String): String
        @JvmStatic external fun getProfile(): String
        @JvmStatic external fun setProfileName(name: String): String
        @JvmStatic external fun setAvatar(bytes: ByteArray, mime: String): String
        @JvmStatic external fun clearAvatar(): String

        // -- Ad filtering (§11) ----------------------------------------------------
        @JvmStatic external fun shouldBlockUrl(url: String): Boolean
        @JvmStatic external fun filterStats(): String
        @JvmStatic external fun refreshFilters(): String

        // -- Self-update (§11) ------------------------------------------------------
        @JvmStatic external fun checkForUpdates(): String
        @JvmStatic external fun downloadUpdate(): String
        @JvmStatic external fun applyUpdate(activity: android.content.Context): String
        @JvmStatic external fun updateStatus(): String

        // -- Session (§8; mirror owns the gate boolean) ------------------------------
        @JvmStatic external fun sessionStatus(): String
        @JvmStatic external fun notifySession(json: String): String
        @JvmStatic external fun logout(): String

        // -- Core→Kotlin events (§6.3, polled future) ---------------------------------
        @JvmStatic external fun pollEvents(): String

        // -- Data (§10 migration, Phase 3 — real; paged where paged today) -----------
        @JvmStatic external fun fetchArtwork(arg: String): String

        // -- Later-phase surface (§6.1 exact signatures; PHASE_2_PLUS until wired).
        // -- Each takes one JSON arg ("" when the call needs none) so this file
        // -- never changes when the real implementation lands.
        @JvmStatic external fun beginLogin(arg: String): String
        @JvmStatic external fun refreshToken(arg: String): String
        @JvmStatic external fun currentUser(arg: String): String
        @JvmStatic external fun getHome(arg: String): String
        @JvmStatic external fun search(arg: String): String
        @JvmStatic external fun getPlaylist(arg: String): String
        @JvmStatic external fun getAlbum(arg: String): String
        @JvmStatic external fun getArtistPage(arg: String): String
        @JvmStatic external fun getLikedTracks(arg: String): String
        @JvmStatic external fun getLibrary(arg: String): String
        @JvmStatic external fun playTrack(arg: String): String
        @JvmStatic external fun playUri(arg: String): String
        @JvmStatic external fun pause(arg: String): String
        @JvmStatic external fun resume(arg: String): String
        @JvmStatic external fun next(arg: String): String
        @JvmStatic external fun prev(arg: String): String
        @JvmStatic external fun seek(arg: String): String
        @JvmStatic external fun setVolume(arg: String): String
        @JvmStatic external fun enqueue(arg: String): String
        @JvmStatic external fun clearQueue(arg: String): String
        @JvmStatic external fun setShuffle(arg: String): String
        @JvmStatic external fun setRepeat(arg: String): String
        @JvmStatic external fun resolveStream(arg: String): String

        // -- SDK path (Phase 5, §9.2): Connect transport against the
        // -- Kotlin-hosted SDK device + state parser + document source.
        @JvmStatic external fun sdkDocument(arg: String): String
        @JvmStatic external fun sdkPlay(arg: String): String
        @JvmStatic external fun sdkPause(arg: String): String
        @JvmStatic external fun sdkSkip(arg: String): String
        @JvmStatic external fun sdkSeek(arg: String): String
        @JvmStatic external fun sdkVolume(arg: String): String
        @JvmStatic external fun sdkParseState(arg: String): String

        // -- Lyrics (Phase 9): LRCLIB via the core (cached, single-flight).
        @JvmStatic external fun fetchLyrics(arg: String): String
    }
}
