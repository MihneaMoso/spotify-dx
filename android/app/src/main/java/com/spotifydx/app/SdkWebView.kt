package com.spotifydx.app

import android.annotation.SuppressLint
import android.util.Log
import android.view.View
import android.webkit.ConsoleMessage
import android.webkit.CookieManager
import android.webkit.JavascriptInterface
import android.webkit.WebChromeClient
import android.webkit.WebView
import android.webkit.WebViewClient
import android.widget.FrameLayout
import androidx.fragment.app.FragmentActivity
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.withTimeoutOrNull
import org.json.JSONObject

/**
 * Hidden platform view hosting the official SDK document (Phase 5, §9.2
 * migration). The document bytes are single-sourced from the core
 * (`player::playback_sdk::SDK_HTML` via `BridgeClient.sdkDocument`); this
 * driver only hosts and drives it: play/pause/track-step/seek/volume map to
 * `window._relay` calls, device readiness and player-state events arrive over
 * the `window.ipc.postMessage` script interface (§8: a proper interface, same
 * message vocabulary as the native host).
 *
 * Cookies are app-wide (CookieManager singleton): the login page's session
 * jar is shared automatically, so the SDK's `credentials: 'include'` token
 * fetch works with no extra plumbing (§12.4 — no second cookie jar).
 *
 * Lazy creation carries over: the view is never built on the engine path
 * that doesn't use it. Audio itself plays inside the WebView; transport
 * *commands* (which track on which device) go through the core's Connect
 * calls (`BridgeClient.sdkPlay/...`), and SDK state events feed back into
 * [PlayerRepository].
 */
class SdkWebViewDriver(
    private val activity: FragmentActivity,
    private val container: FrameLayout,
) {
    companion object {
        const val TAG = "SpotifyDxSdk"
        /** Base URL gives the document the open.spotify.com origin so the
         * session cookies attach to its token fetch. */
        const val BASE_URL = "https://open.spotify.com/"
        private const val READY_TIMEOUT_MS = 15_000L
    }

    private var webView: WebView? = null
    var deviceReady: Boolean = false
        private set
    var deviceId: String? = null
        private set

    /** Fired on the main thread when the SDK registers its device. */
    var onDeviceReady: ((String) -> Unit)? = null
    /** Fired on the main thread with the raw `player_state_changed` payload. */
    var onPlayerState: ((String) -> Unit)? = null

    private var readySignal = CompletableDeferred<String>()

    inner class Ipc {
        /** Runs on a WebView background thread — marshal to main. */
        @JavascriptInterface
        fun postMessage(msg: String) {
            activity.runOnUiThread { handleMessage(msg) }
        }
    }

    private fun handleMessage(msg: String) {
        val o = runCatching { JSONObject(msg) }.getOrNull() ?: run {
            Log.w(TAG, "ipc: unparseable message ${msg.take(80)}")
            return
        }
        when (o.optString("type", "")) {
            "ready" -> {
                val id = o.optString("device_id", "")
                if (id.isEmpty()) {
                    Log.w(TAG, "ipc: ready without device_id")
                    return
                }
                deviceId = id
                deviceReady = true
                if (!readySignal.isCompleted) readySignal.complete(id)
                Log.i(TAG, "SDK device ready: ${id.take(12)}…")
                onDeviceReady?.invoke(id)
            }
            "not_ready" -> {
                deviceReady = false
                deviceId = null
                readySignal = CompletableDeferred()
                Log.i(TAG, "SDK device lost")
            }
            "state" -> {
                val payload = o.optJSONObject("payload")?.toString() ?: "{}"
                Log.d(TAG, "SDK state: ${payload.take(120)}")
                onPlayerState?.invoke(payload)
            }
            "token_refresh" -> {
                // Self-managed inside the document (cookies); the bridge
                // mirror owns its own token. Log only.
                Log.d(TAG, "SDK token refreshed (anon=${o.optBoolean("isAnon", true)})")
            }
            "auth_error", "init_error", "token_error" -> {
                val m = o.optString("message", o.optString("msg", "unknown SDK error"))
                Log.w(TAG, "SDK error: $m")
                ToastBus.error("Playback error: $m")
            }
            else -> Log.d(TAG, "ipc: ignored type ${o.optString("type", "?")}")
        }
    }

    /**
     * Build the hidden view (main thread — WebView requirement) and boot the
     * SDK document. Idempotent; safe to call before every SDK operation.
     */
    @SuppressLint("SetJavaScriptEnabled")
    suspend fun ensure(): Boolean {
        if (webView != null) return true
        val html = BridgeClient.sdkDocument().getOrNull() ?: run {
            Log.w(TAG, "SDK document fetch failed")
            return false
        }
        // WebView construction must happen on the main thread.
        if (android.os.Looper.myLooper() != android.os.Looper.getMainLooper()) {
            Log.w(TAG, "ensure() off main thread; refusing (WebView requirement)")
            return false
        }
        readySignal = CompletableDeferred()
        val wv = WebView(activity).apply {
            visibility = View.GONE
            settings.javaScriptEnabled = true
            settings.domStorageEnabled = true
            // Hidden view: no user gesture will ever arrive.
            settings.mediaPlaybackRequiresUserGesture = false
            addJavascriptInterface(Ipc(), "ipc")
            webViewClient = object : WebViewClient() {
                // Fail-closed navigation (§12.7): the SDK document has no
                // top-level navigations; keep everything in-view.
                override fun shouldOverrideUrlLoading(
                    view: WebView,
                    request: android.webkit.WebResourceRequest,
                ): Boolean = true
            }
            webChromeClient = object : WebChromeClient() {
                override fun onConsoleMessage(m: ConsoleMessage): Boolean {
                    Log.d(TAG, "sdk-console: ${m.message().take(160)}")
                    return true
                }
            }
        }
        CookieManager.getInstance().setAcceptThirdPartyCookies(wv, true)
        container.addView(wv, FrameLayout.LayoutParams(1, 1))
        webView = wv
        wv.loadDataWithBaseURL(BASE_URL, html, "text/html", "utf-8", null)
        Log.i(TAG, "SDK document loading")
        return true
    }

    /** Device id, waiting for `ready` up to the timeout. Null = not ready. */
    suspend fun awaitDevice(timeoutMs: Long = READY_TIMEOUT_MS): String? {
        deviceId?.takeIf { deviceReady }?.let { return it }
        return withTimeoutOrNull(timeoutMs) { readySignal.await() }
    }

    // -- Relay: client-side transport (resume/pause/step in place) -------------
    fun play() = relay("play")
    fun pause() = relay("pause")
    fun next() = relay("next")
    fun prev() = relay("prev")
    fun seek(ms: Long) = relay("seek", ms.toString())
    fun setVolume(v: Float) = relay("volume", v.toString())

    private fun relay(call: String, arg: String = "") {
        val wv = webView
        if (wv == null) {
            Log.d(TAG, "SDK relay($call) ignored: driver parked")
            return
        }
        wv.evaluateJavascript("try{window._relay.$call($arg)}catch(e){}", null)
    }

    fun shutdown() {
        container.removeAllViews()
        webView?.destroy()
        webView = null
        deviceReady = false
        deviceId = null
        readySignal = CompletableDeferred()
    }

    fun attachHidden() {
        val wv = webView ?: return
        if (wv.parent == null) {
            wv.visibility = View.GONE
            container.addView(wv, FrameLayout.LayoutParams(1, 1))
        }
    }
}
