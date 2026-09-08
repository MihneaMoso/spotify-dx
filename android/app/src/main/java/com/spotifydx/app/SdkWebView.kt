package com.spotifydx.app

import android.annotation.SuppressLint
import android.util.Log
import android.view.View
import android.webkit.JavascriptInterface
import android.webkit.WebView
import android.webkit.WebViewClient
import android.widget.FrameLayout
import androidx.fragment.app.FragmentActivity

/**
 * Hidden platform view hosting the official SDK document (Phase 5, §9.2
 * migration). Same document the native build bootstraps (vendor SDK
 * bootstrap, token callback, relay object, state/error listeners), driven
 * from Kotlin instead of the core: play/pause/track-step/seek/volume map to
 * relay calls, device readiness and player-state events map to the playback
 * event stream.
 *
 * Lazy creation carries over: this view is never built on the engine path
 * that doesn't use it. Until Phase 5 wires the SDK document + device
 * handoff, the view stays parked and every driver call is a logged no-op —
 * the open-engine path (§9.6) is the active one.
 */
class SdkWebViewDriver(
    private val activity: FragmentActivity,
    private val container: FrameLayout,
) {
    companion object {
        const val TAG = "SpotifyDxSdk"
    }

    private var webView: WebView? = null
    var deviceReady: Boolean = false
        private set

    inner class Relay {
        @JavascriptInterface
        fun onDeviceReady(deviceId: String) {
            deviceReady = true
            Log.i(TAG, "SDK device ready: ${deviceId.take(12)}…")
        }

        @JavascriptInterface
        fun onPlayerState(json: String) {
            Log.d(TAG, "SDK state: ${json.take(120)}")
        }

        @JavascriptInterface
        fun onError(msg: String) {
            Log.w(TAG, "SDK error: $msg")
            ToastBus.error("Playback error: $msg")
        }
    }

    /** Build the hidden view. No-op until the Phase 5 SDK document lands. */
    @SuppressLint("SetJavaScriptEnabled")
    fun ensure() {
        if (webView != null) return
        // TODO(Phase 5): load the vendor SDK bootstrap document (same bytes
        //  the core serves from playback_sdk) with the session cookies, then
        //  drive play/pause/track-step/seek/volume through window._relay.
        Log.i(TAG, "SDK driver parked (Phase 5 wiring pending)")
    }

    fun play() = relay("play")
    fun pause() = relay("pause")
    fun nextTrack() = relay("nextTrack")
    fun prevTrack() = relay("prevTrack")
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
    }

    fun attachHidden() {
        val wv = webView ?: return
        if (wv.parent == null) {
            wv.visibility = View.GONE
            container.addView(wv, FrameLayout.LayoutParams(1, 1))
        }
    }
}
