package com.spotifydx.app

import android.annotation.SuppressLint
import android.graphics.Bitmap
import android.os.Handler
import android.os.Looper
import android.util.Log
import android.view.ViewGroup
import android.webkit.CookieManager
import android.webkit.JavascriptInterface
import android.webkit.WebChromeClient
import android.webkit.WebResourceRequest
import android.webkit.WebView
import android.webkit.WebViewClient
import android.widget.FrameLayout
import androidx.fragment.app.FragmentActivity
import org.json.JSONObject

/**
 * Shared owner of the activity-hosted session WebView (§8 migration). The
 * Activity hosts it fullscreen layered above the gate (the interface
 * underneath is never touched, reparented, or rebuilt — §12.3). Pages are
 * shown/hidden/detached in place; realized views are never moved between
 * containers.
 *
 * This class owns MECHANICS only: construction, the shared WebViewClient
 * (containment, error surfacing, injection, revive signals), IPC dispatch,
 * visibility primitives, parking, and teardown. Intent lives in the two
 * facades: [LoginPage] (visible interactive sign-in) and [RefreshChannel]
 * (hidden silent refresh). Callers must use a facade, never this class
 * directly — except Activity lifecycle (`destroy`) and the facades.
 *
 * Capture keeps its layered methods in priority order (see [CaptureJs]);
 * only non-anonymous tokens satisfy login. On capture the page stays alive
 * hidden as the refresh channel and is parked on a blank document when idle
 * (cookies/storage survive; refresh revives on demand, ~hourly).
 *
 * Popup/new-window containment and external-scheme bounce-back match the
 * current spec exactly: new-window requests are denied fail-closed,
 * non-web schemes bounce back to sign-in, navigation URLs are traced in
 * diagnostics.
 */
class SessionPage(
    private val activity: FragmentActivity,
    private val container: FrameLayout,
) {
    companion object {
        const val SIGN_IN_URL =
            "https://accounts.spotify.com/en/login?continue=https%3A%2F%2Fopen.spotify.com%2F"
        const val PLAYER_URL = "https://open.spotify.com/"
        const val TAG = "SpotifyDxLogin"
    }

    private var webView: WebView? = null
    private var captured = false
    private val main = Handler(Looper.getMainLooper())

    /** One-shot hook fired from the shared onPageFinished (revive flow). */
    internal var reviveHook: (() -> Unit)? = null

    /** Flight-scoped revive signals, reset at every revive start (main thread). */
    internal var reviveArmed = false
    internal var reviveLoaded = false
    internal var reviveError = false
    internal var reviveCaptured = false

    /** Script-interface receiver: every call arrives off the UI thread. */
    inner class JsBridge {
        @JavascriptInterface
        fun postMessage(raw: String) {
            main.post { handleIpc(raw) }
        }

        @JavascriptInterface
        fun log(msg: String) {
            Log.d(TAG, "page: $msg")
        }
    }

    private fun handleIpc(raw: String) {
        val o = runCatching { JSONObject(raw) }.getOrNull() ?: return
        when (o.optString("type")) {
            "logged_in" -> {
                val token = o.optString("token", "")
                val expires = o.optLong("expiresMs", 0)
                if (token.isNotEmpty()) {
                    onCaptured(token, expires)
                } else {
                    // DOM fallback with no token: proceed so the session page
                    // can mint one from the shared cookies (Phase 2 core).
                    Log.i(TAG, "login settled without token; parking session page")
                    park()
                    SessionRepository.refresh()
                }
            }
            "token_refresh_result" -> {
                val token = o.optString("token", "")
                val expires = o.optLong("expiresMs", 0)
                if (token.isNotEmpty() && !o.optBoolean("isAnon", false)) {
                    reviveCaptured = true
                    SessionRepository.notifyCaptured(token, expires, null)
                }
            }
            "token_error" -> Log.w(TAG, "token refresh error: ${o.optString("msg")}")
            "token_debug" -> Log.d(TAG, "capture: ${o.optString("msg")}")
        }
    }

    private fun onCaptured(token: String, expiresMs: Long) {
        reviveCaptured = true
        if (captured) {
            // Late captures keep the session fresh (same as `store()` post-login).
            SessionRepository.notifyCaptured(token, expiresMs, null)
            return
        }
        captured = true
        SessionRepository.notifyCaptured(token, expiresMs, null)
        // Hide but keep alive as the refresh channel, then park when idle.
        main.postDelayed({ park() }, 2_000)
        hide()
    }

    /** Build + show the fullscreen login page. Idempotent (re-entry guard). */
    @SuppressLint("SetJavaScriptEnabled")
    fun show(url: String = SIGN_IN_URL) {
        val already = webView != null
        val wv = ensure()
        if (already) {
            // A live page may be showing a stale step (e.g. post-expiry
            // re-login landing on the player instead of sign-in): navigate
            // when the caller asked for a different URL.
            if (wv.url != url) wv.loadUrl(url)
            container.visibility = android.view.View.VISIBLE
            return
        }
        captured = false
        container.visibility = android.view.View.VISIBLE
        wv.loadUrl(url)
    }

    /**
     * Build the session page if absent, WITHOUT showing it (stays
     * hidden/parked until an explicit show or a revive navigation). Lets
     * cold-boot silent refresh mint from disk cookies exactly like the
     * backgrounded case — previously a null page failed revive instantly.
     */
    @SuppressLint("SetJavaScriptEnabled")
    internal fun ensure(): WebView {
        webView?.let { return it }
        val wv = WebView(activity)
        wv.layoutParams = FrameLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT,
            ViewGroup.LayoutParams.MATCH_PARENT,
        )
        val cm = CookieManager.getInstance()
        cm.setAcceptCookie(true)
        cm.acceptThirdPartyCookies(wv)
        val s = wv.settings
        s.javaScriptEnabled = true
        s.domStorageEnabled = true
        s.mediaPlaybackRequiresUserGesture = false
        s.loadWithOverviewMode = true
        s.useWideViewPort = true
        wv.addJavascriptInterface(JsBridge(), "SpotifyDx")
        wv.webViewClient = object : WebViewClient() {
            override fun shouldOverrideUrlLoading(view: WebView, req: WebResourceRequest): Boolean {
                val url = req.url.toString()
                Log.d(TAG, "nav: $url")
                val scheme = req.url.scheme?.lowercase()
                if (scheme != "http" && scheme != "https") {
                    // External-scheme bounce-back: stay in-view on sign-in.
                    Log.w(TAG, "bouncing external scheme: $scheme")
                    view.loadUrl(SIGN_IN_URL)
                    return true
                }
                // Same-window navigation (containment = never leave the view).
                return false
            }

            override fun onPageStarted(view: WebView, url: String, favicon: Bitmap?) {
                injectCapture(view)
            }

            override fun onPageFinished(view: WebView, url: String) {
                injectCapture(view)
                // Persist cookies promptly: silent restore depends on them.
                CookieManager.getInstance().flush()
                // Revive-flight signal: an http(s) document actually
                // rendered (login wall included — that IS the loaded
                // signal; tokenlessness is read at flight exit).
                if (reviveArmed && url.startsWith("http")) reviveLoaded = true
                // Revive-flow hook (one-shot); the shared client — with its
                // containment, error surfacing, and injection — stays installed.
                reviveHook?.let { hook ->
                    reviveHook = null
                    hook()
                }
            }

            // Network failures (airplane mode, captive portal) surface as a
            // toast + the gate's retry control — the same errors as today,
            // since the page itself is genuine Spotify UI.
            override fun onReceivedError(
                view: WebView,
                request: WebResourceRequest,
                error: android.webkit.WebResourceError,
            ) {
                if (!request.isForMainFrame) return
                // Revive-flight signal: a main-frame load error (airplane
                // mode, captive portal, cert failure) means "page never
                // rendered" — TRANSIENT, never proof of a dead session
                // (onPageFinished still fires for failed loads, so loaded
                // alone can't carry that meaning).
                if (reviveArmed) reviveError = true
                Log.w(TAG, "page error: ${error.errorCode} ${request.url}")
                ToastBus.error("No connection — check your network and retry.")
            }
        }
        wv.webChromeClient = object : WebChromeClient() {
            // Fail-closed popups: no second window is ever created.
            override fun onCreateWindow(
                view: WebView,
                isDialog: Boolean,
                isUserGesture: Boolean,
                resultMsg: android.os.Message,
            ): Boolean {
                Log.w(TAG, "denied popup window request")
                return false
            }

            override fun onProgressChanged(view: WebView, progress: Int) {
                if (progress % 25 == 0) Log.d(TAG, "load progress: $progress")
            }

            override fun onReceivedTitle(view: WebView, title: String) {
                Log.d(TAG, "title: ${title.take(80)}")
            }
        }
        container.removeAllViews()
        container.addView(wv)
        webView = wv
        return wv
    }

    private fun injectCapture(view: WebView) {
        val url = view.url ?: return
        if (!url.startsWith("http")) return
        view.evaluateJavascript(CaptureJs.IPC_SHIM, null)
        view.evaluateJavascript(CaptureJs.POLL_JS, null)
    }

    fun hide() {
        container.visibility = android.view.View.GONE
    }

    /** Park on a blank document: cookies/storage survive, CPU goes idle. */
    fun park() {
        CookieManager.getInstance().flush()
        webView?.loadUrl("about:blank")
        hide()
    }

    /** Forget a captured login so the next interactive flow starts clean. */
    internal fun resetCapture() {
        captured = false
    }

    /** Detach + destroy the realized view (logout teardown half). */
    internal fun teardownView() {
        reviveHook = null
        container.visibility = android.view.View.GONE
        container.removeAllViews()
        webView?.apply {
            stopLoading()
            clearHistory()
            removeJavascriptInterface("SpotifyDx")
            destroy()
        }
        webView = null
    }

    /** Best-effort cookie wipe (logout teardown half; credentials authoritative). */
    internal fun clearCookies() {
        runCatching {
            CookieManager.getInstance().removeAllCookies(null)
            CookieManager.getInstance().flush()
        }
    }

    internal fun post(block: () -> Unit) {
        main.post(block)
    }

    internal fun postDelayed(block: () -> Unit, ms: Long) {
        main.postDelayed(block, ms)
    }

    fun destroy() {
        container.removeAllViews()
        webView?.destroy()
        webView = null
    }
}
