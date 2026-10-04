package com.spotifydx.app

import android.webkit.CookieManager
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONObject
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec

/**
 * Direct token mint (§5.4 fast path). The session cookies already live in the
 * shared WebView cookie jar, and `/api/token` needs only those cookies plus
 * a locally-computed TOTP (same key/params as [CaptureJs] `POLL_JS`) — no
 * page load. A full open.spotify.com load costs 10–15s per revive attempt
 * (the expiry retry hell); this answers in ~1s. Any failure is silent
 * (null) — the caller falls back to the WebView revive, which stays the
 * authoritative path (page server time, DOM signals).
 */
object SessionTokenFetch {
    /** Full deobfuscated decimal key — must match `POLL_JS` exactly. */
    const val TOTP_KEY = "376136387538459893883312310911992847112448894410210511297108"
    private const val TOTP_VER = "61"

    data class Captured(val token: String, val expiresAtMs: Long)

    suspend fun fetchDirect(): Captured? = withContext(Dispatchers.IO) {
        runCatching { fetch() }.getOrNull()
    }

    private fun fetch(): Captured? {
        val cookies = CookieManager.getInstance()
            .getCookie("https://open.spotify.com")
            ?.takeIf { it.isNotEmpty() } ?: return null
        val now = System.currentTimeMillis()
        // Local clock for both TOTP slots (the page prefers its server time
        // when available; phone clocks are NTP-synced — and any skew failure
        // just falls back to the WebView revive, never a user error).
        val totp = totp(now)
        val url = java.net.URL(
            "https://open.spotify.com/api/token?reason=transport&productType=web_player" +
                "&totp=$totp&totpServer=$totp&totpVer=$TOTP_VER",
        )
        val conn = (url.openConnection() as java.net.HttpURLConnection).apply {
            // Tight budgets: this is the fast path before the WebView
            // revive — a hanging network must fall through quickly, not
            // stack its timeout on top of the 15s revive.
            connectTimeout = 5_000
            readTimeout = 5_000
            setRequestProperty("Cookie", cookies)
            setRequestProperty("Accept", "application/json")
            setRequestProperty(
                "User-Agent",
                "Mozilla/5.0 (Linux; Android 14) AppleWebKit/537.36" +
                    " (KHTML, like Gecko) Chrome/126.0 Mobile Safari/537.36",
            )
            setRequestProperty("Origin", "https://open.spotify.com")
            setRequestProperty("Referer", "https://open.spotify.com/")
        }
        try {
            if (conn.responseCode != 200) return null
            val body = conn.inputStream.bufferedReader().use { it.readText() }
            val o = JSONObject(body)
            val token = o.optString("accessToken", "")
            if (token.isEmpty() || o.optBoolean("isAnonymous", true)) return null
            return Captured(token, o.optLong("accessTokenExpirationTimestampMs", 0))
        } finally {
            conn.disconnect()
        }
    }

    /** RFC 6238 TOTP (HMAC-SHA1, 6 digits, 30s period) over the page key. */
    internal fun totp(nowMs: Long): String {
        val counter = nowMs / 30_000L
        val msg = ByteArray(8) { i -> (counter ushr (56 - 8 * i)).toByte() }
        val mac = Mac.getInstance("HmacSHA1")
        mac.init(SecretKeySpec(TOTP_KEY.toByteArray(Charsets.US_ASCII), "HmacSHA1"))
        val h = mac.doFinal(msg)
        val o = h[h.size - 1].toInt() and 0x0f
        val bin = ((h[o].toInt() and 0x7f) shl 24) or
            ((h[o + 1].toInt() and 0xff) shl 16) or
            ((h[o + 2].toInt() and 0xff) shl 8) or
            (h[o + 3].toInt() and 0xff)
        return String.format("%06d", bin % 1_000_000)
    }
}
