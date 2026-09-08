package com.spotifydx.app

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONObject

/**
 * Typed errors mirroring the core's error hierarchy
 * (docs/ARCHITECTURE.md §15). The Kotlin side maps every bridge failure to
 * one of these — never bare strings across the UI boundary.
 */
sealed interface BridgeError {
    /** Core is healthy; the call's own phase has not landed yet (§6.1). */
    data class NotWired(val phase: String, val message: String) : BridgeError
    /** Session page revive required (stale/missing token, §8/§5.4). */
    data class NeedsPage(val message: String) : BridgeError
    /** Token rejected server-side: first-class expiry transition to the gate. */
    data class SessionExpired(val message: String) : BridgeError
    /** Fail-fast rate limit: banner + timed auto-retry, never a spinner. */
    data class RateLimited(val message: String) : BridgeError
    /** Typed core failure (INVALID_ARGS / IO / NET / BRIDGE_*). */
    data class Core(val code: String, val message: String) : BridgeError
    /** The JVM↔native contract itself broke (unparseable envelope). */
    data class Protocol(val message: String) : BridgeError
}

/** Decoded `{"ok","code","message","data"}` envelope. */
data class Envelope(
    val ok: Boolean,
    val code: String?,
    val message: String?,
    /** Raw `data` JSON (object/array/string), or null. */
    val data: String?,
)

fun parseEnvelope(raw: String): Envelope {
    val o = JSONObject(raw)
    // optString(x, null) trips a Java-nullability warning; read explicitly.
    fun opt(key: String): String? = if (o.isNull(key)) null else o.getString(key)
    return Envelope(
        ok = o.optBoolean("ok", false),
        code = opt("code").takeUnless { it.isNullOrEmpty() },
        message = opt("message"),
        data = if (o.isNull("data")) null else o.opt("data")?.toString(),
    )
}

/**
 * Coroutine wrapper over [CoreBridge]. Blocking calls run on
 * `Dispatchers.IO` (§12.1 — never the interface thread); results surface as
 * Kotlin [Result] with [BridgeError] failures.
 */
object BridgeClient {
    suspend fun initCore(filesDir: String, cacheDir: String): Result<JSONObject> =
        callData { CoreBridge.initCore(filesDir, cacheDir) }

    /** Refuses loudly with a diagnostic on mismatch (§6, §12.7). */
    fun checkVersion() {
        val v = CoreBridge.bridgeVersion()
        if (v != CoreBridge.EXPECTED_VERSION) {
            throw BridgeException(
                BridgeError.Protocol(
                    "bridge version mismatch: native=$v expected=${CoreBridge.EXPECTED_VERSION}",
                ),
            )
        }
    }

    suspend fun getSettings(): Result<JSONObject> = callData { CoreBridge.getSettings() }
    suspend fun setSettings(json: String): Result<JSONObject> = callData { CoreBridge.setSettings(json) }
    suspend fun getProfile(): Result<JSONObject> = callData { CoreBridge.getProfile() }
    suspend fun setProfileName(name: String): Result<JSONObject> =
        callData { CoreBridge.setProfileName(name) }
    suspend fun setAvatar(bytes: ByteArray, mime: String): Result<JSONObject> =
        callData { CoreBridge.setAvatar(bytes, mime) }
    suspend fun clearAvatar(): Result<JSONObject> = callData { CoreBridge.clearAvatar() }

    /** Cheap-sync; safe from any thread. Fail-open (false) on protocol error. */
    fun shouldBlockUrl(url: String): Boolean =
        runCatching { CoreBridge.shouldBlockUrl(url) }.getOrDefault(false)

    suspend fun filterStats(): Result<JSONObject> = callData { CoreBridge.filterStats() }
    suspend fun refreshFilters(): Result<String> = callString { CoreBridge.refreshFilters() }

    suspend fun checkForUpdates(): Result<JSONObject> = callData { CoreBridge.checkForUpdates() }
    suspend fun downloadUpdate(): Result<JSONObject> = callData { CoreBridge.downloadUpdate() }
    suspend fun updateStatus(): Result<String> = callString { CoreBridge.updateStatus() }
    suspend fun applyUpdate(activity: android.content.Context): Result<String> =
        callString { CoreBridge.applyUpdate(activity) }

    suspend fun sessionStatus(): Result<JSONObject> = callData { CoreBridge.sessionStatus() }
    suspend fun notifySession(json: String): Result<String> =
        callString { CoreBridge.notifySession(json) }
    suspend fun logout(): Result<String> = callString { CoreBridge.logout() }

    // -- Phase 2 session surface (real) -------------------------------------------
    suspend fun beginLogin(): Result<JSONObject> = callData { CoreBridge.beginLogin("") }
    suspend fun refreshToken(): Result<JSONObject> = callData { CoreBridge.refreshToken("") }
    suspend fun currentUser(): Result<JSONObject> = callData { CoreBridge.currentUser("") }

    suspend fun pollEvents(): Result<List<EventBus.CoreEvent>> = withContext(Dispatchers.IO) {
        runCatching {
            val env = parseEnvelope(CoreBridge.pollEvents())
            if (!env.ok) throw toException(env)
            val arr = org.json.JSONArray(env.data ?: "[]")
            List(arr.length()) { i ->
                val o = arr.getJSONObject(i)
                EventBus.CoreEvent(o.getString("kind"), o.get("payload").toString())
            }
        }
    }

    // -- Later-phase surface: identical call shape, PHASE_2_PLUS until wired. --
    // -- Browse/data calls land in Phase 3 (not Phase 2), labeled as such. --
    suspend fun getHome(): Result<JSONObject> = callData("Phase 3") { CoreBridge.getHome("") }
    suspend fun search(query: String): Result<JSONObject> =
        callData("Phase 3") { CoreBridge.search(JSONObject().put("query", query).toString()) }

    // -- Phase 3 data surface (real; §6.2 paged contract) ---------------------------
    suspend fun playlist(id: String): Result<JSONObject> =
        callData("Phase 3") { CoreBridge.getPlaylist(arg(id)) }
    suspend fun album(id: String): Result<JSONObject> =
        callData("Phase 3") { CoreBridge.getAlbum(arg(id)) }
    suspend fun artistPage(id: String): Result<JSONObject> =
        callData("Phase 3") { CoreBridge.getArtistPage(arg(id)) }
    suspend fun likedTracks(limit: Int, offset: Int): Result<JSONObject> =
        callData("Phase 3") {
            CoreBridge.getLikedTracks(
                JSONObject().put("limit", limit).put("offset", offset).toString(),
            )
        }
    suspend fun library(kind: String, limit: Int, offset: Int): Result<JSONObject> =
        callData("Phase 3") {
            CoreBridge.getLibrary(
                JSONObject().put("kind", kind).put("limit", limit).put("offset", offset).toString(),
            )
        }

    /** Resolve a track to a playable URL (Phase 4, open engine). */
    suspend fun resolveStream(trackJson: String): Result<JSONObject> =
        callData("Phase 4") { CoreBridge.resolveStream(trackJson) }

    /** Artwork bytes (base64) through the core's disk cache + filter gate. */
    suspend fun fetchArtwork(url: String): Result<String> = withContext(Dispatchers.IO) {
        runCatching {
            val env = parseEnvelope(
                CoreBridge.fetchArtwork(JSONObject().put("url", url).toString()),
            )
            if (!env.ok) throw toException(env, "Phase 3")
            // `data` is a JSON-encoded string (quoted base64); unquote it.
            env.data?.removeSurrounding("\"")
                ?: throw BridgeException(BridgeError.Protocol("artwork payload missing"))
        }
    }

    private fun arg(id: String): String = JSONObject().put("id", id).toString()

    private suspend fun callData(phase: String = "Phase 2+", block: () -> String): Result<JSONObject> =
        withContext(Dispatchers.IO) {
            runCatching {
                val env = parseEnvelope(block())
                if (!env.ok) throw toException(env, phase)
                // Collection legs return bare arrays; normalize to {items}.
                val data = env.data ?: "{}"
                if (data.trimStart().startsWith("[")) {
                    JSONObject().put("items", org.json.JSONArray(data))
                } else {
                    JSONObject(data)
                }
            }
        }

    private suspend fun callString(block: () -> String): Result<String> =
        withContext(Dispatchers.IO) {
            runCatching {
                val env = parseEnvelope(block())
                if (!env.ok) throw toException(env)
                // `data` may be a JSON string or an object; return it raw.
                env.data ?: ""
            }
        }

    /** Shared by MusicRepository: same codes, Phase 3 label. */
    internal fun toException(env: Envelope, phase: String = "Phase 2+"): BridgeException =
        when (env.code) {
            "PHASE_2_PLUS" -> BridgeException(
                BridgeError.NotWired(phase, env.message ?: "not wired yet"),
            )
            "NEEDS_PAGE", "NO_PAGE" -> BridgeException(
                BridgeError.NeedsPage(env.message ?: "session page required"),
            )
            "SESSION_EXPIRED" -> BridgeException(
                BridgeError.SessionExpired(env.message ?: "session expired"),
            )
            "RATE_LIMITED" -> BridgeException(
                BridgeError.RateLimited(env.message ?: "rate limited"),
            )
            else -> BridgeException(
                BridgeError.Core(env.code ?: "UNKNOWN", env.message ?: "bridge error"),
            )
        }
}

class BridgeException(val error: BridgeError) : Exception(error.toString())
