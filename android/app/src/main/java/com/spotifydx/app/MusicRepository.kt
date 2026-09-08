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
    suspend fun home(): Result<JSONObject> = BridgeClient.getHome()

    suspend fun search(query: String): Result<JSONObject> = BridgeClient.search(query)

    suspend fun playlist(id: String): Result<JSONObject> = BridgeClient.playlist(id)

    suspend fun album(id: String): Result<JSONObject> = BridgeClient.album(id)

    suspend fun artistPage(id: String): Result<JSONObject> = BridgeClient.artistPage(id)

    suspend fun likedTracks(limit: Int, offset: Int): Result<JSONObject> =
        BridgeClient.likedTracks(limit, offset)

    suspend fun library(kind: String, limit: Int, offset: Int): Result<JSONObject> =
        BridgeClient.library(kind, limit, offset)

    suspend fun artwork(url: String): Result<String> = BridgeClient.fetchArtwork(url)

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
