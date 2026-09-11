package com.spotifydx.app

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONObject

/**
 * Screen data access (§10 migration). One bridge call per screen data need,
 * same pagination, same partial-failure tolerance (a failing leg yields
 * defaults, never a crash). No retry loops around bridge calls — rate-limit
 * behavior is contractual (bounded wait + single retry in core, banner +
 * timed auto-retry in the ViewModel).
 */
object MusicRepository {
    /**
     * Session memory tier (built-in `android.util.LruCache`, TTL-bounded):
     * repeat screen loads skip the JNI round-trip + re-parse within the
     * window. Only successes cache (never errors — same invariant as the
     * core store); failures always run the fetch so session-expiry and
     * rate-limit handling in `withSessionCheck` still see them. The core's
     * disk SWR stays the cross-restart layer; [clear] runs on logout.
     */
    private data class Entry(val json: String, val fetchedAtMs: Long)
    private const val TTL_MS = 5 * 60_000L
    private val mem = object : android.util.LruCache<String, Entry>(32) {}

    // Empty-but-successful payloads (e.g. a failed fan-out leg that
    // defaulted) must NOT cache — they'd pin an empty screen until TTL.
    // `fetch` stays last so call sites keep trailing-lambda syntax.
    private suspend fun cached(
        key: String,
        isValid: (JSONObject) -> Boolean = { true },
        fetch: suspend () -> Result<JSONObject>,
    ): Result<JSONObject> {
        val now = System.currentTimeMillis()
        mem.get(key)?.let { e ->
            if (now - e.fetchedAtMs < TTL_MS) {
                return Result.success(JSONObject(e.json))
            }
            mem.remove(key)
        }
        val res = fetch()
        res.getOrNull()?.let { json ->
            if (isValid(json)) mem.put(key, Entry(json.toString(), now))
        }
        return res
    }

    /** Drop all session data (logout / account switch). */
    fun clear() = mem.evictAll()

    suspend fun home(): Result<JSONObject> = cached(
        "home",
        isValid = { json ->
            (json.optJSONArray("playlists")?.length() ?: 0) > 0 ||
                (json.optJSONArray("liked_tracks")?.length() ?: 0) > 0
        },
        fetch = { BridgeClient.getHome() },
    )

    suspend fun search(query: String): Result<JSONObject> =
        cached("search:${query.trim().lowercase()}") { BridgeClient.search(query) }

    suspend fun playlist(id: String): Result<JSONObject> =
        cached("playlist:$id") { BridgeClient.playlist(id) }

    suspend fun album(id: String): Result<JSONObject> =
        cached("album:$id") { BridgeClient.album(id) }

    suspend fun artistPage(id: String): Result<JSONObject> =
        cached("artist:$id") { BridgeClient.artistPage(id) }

    suspend fun likedTracks(limit: Int, offset: Int): Result<JSONObject> =
        cached("liked:$limit:$offset") { BridgeClient.likedTracks(limit, offset) }

    suspend fun library(kind: String, limit: Int, offset: Int): Result<JSONObject> =
        cached("library:$kind:$limit:$offset") { BridgeClient.library(kind, limit, offset) }

    suspend fun artwork(url: String): Result<String> = BridgeClient.fetchArtwork(url)

    /** Lyrics for [track] (null when none exist — a normal outcome). */
    suspend fun lyrics(track: Track): Result<JSONObject?> {
        if (track.artistNames.isEmpty() || track.name.isEmpty()) {
            return Result.success(null)
        }
        return BridgeClient.fetchLyrics(
            track.artists.firstOrNull() ?: "",
            track.name,
            track.albumName,
            track.durationMs,
        ).map { json -> json.takeIf { it.optBoolean("found", false) } }
    }

    /**
     * Resolve [track] to `{url, format, provider}`. Sends the core-shaped
     * Track object (id/name required, artists as [{name}] refs, album with
     * images) — play-with-metadata, zero network beyond resolution.
     */
    suspend fun resolveStream(track: Track): Result<JSONObject> {
        val artists = org.json.JSONArray()
        track.artists.forEach {
            artists.put(org.json.JSONObject().put("id", "").put("name", it))
        }
        val images = org.json.JSONArray()
        if (track.coverUrl.isNotEmpty()) {
            images.put(org.json.JSONObject().put("url", track.coverUrl))
        }
        val payload = JSONObject()
            .put(
                "track", JSONObject()
                    .put("id", track.id)
                    .put("name", track.name)
                    .put("duration_ms", track.durationMs)
                    .put("artists", artists)
                    .put(
                        "album", JSONObject()
                            .put("id", "")
                            .put("name", track.albumName)
                            .put("images", images),
                    )
                    .put("uri", track.uri),
            ).toString()
        return BridgeClient.resolveStream(payload)
    }

    /** Session errors bubble to the gate; data errors stay page-local. */
    fun isSessionFailure(e: Throwable?): Boolean =
        when ((e as? BridgeException)?.error) {
            is BridgeError.SessionExpired, is BridgeError.NeedsPage -> true
            else -> false
        }

    suspend fun <T> withSessionCheck(block: suspend () -> Result<T>): Result<T> {
        val res = withContext(Dispatchers.IO) { block() }
        if (isSessionFailure(res.exceptionOrNull())) {
            SessionRepository.logout()
        }
        return res
    }
}
