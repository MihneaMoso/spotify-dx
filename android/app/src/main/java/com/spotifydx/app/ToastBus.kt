package com.spotifydx.app

import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.asSharedFlow

/**
 * Error-to-toast bus (§10.1 shell, §15 ARCHITECTURE): user-facing errors with
 * timed auto-dismiss (each new toast cancels the previous dismiss timer —
 * see MainActivity.showToast) and manual dismiss.
 */
object ToastBus {
    data class Toast(val message: String)

    private val _toasts = MutableSharedFlow<Toast>(extraBufferCapacity = 8)
    val toasts: SharedFlow<Toast> = _toasts.asSharedFlow()

    fun error(message: String) {
        _toasts.tryEmit(Toast(message))
    }

    /** Resource-backed toast (keeps user copy in strings.xml, not logic). */
    fun errorRes(@androidx.annotation.StringRes resId: Int, vararg args: Any) {
        val msg = runCatching { AppState.ctx().getString(resId, *args) }.getOrNull()
        error(msg ?: "Something went wrong")
    }

    fun fromBridge(e: Throwable) {
        val msg = when (val err = (e as? BridgeException)?.error) {
            is BridgeError.NotWired -> "Not available yet (${err.phase})"
            is BridgeError.NeedsPage -> "Session expired — signing you back in…"
            is BridgeError.SessionExpired -> "Session expired — please sign in again"
            is BridgeError.RateLimited -> "Spotify's API is temporarily limiting requests — retrying automatically."
            is BridgeError.Core -> if (err.code == "PREMIUM_REQUIRED") {
                "That needs Spotify Premium — playing via the open engine instead."
            } else {
                err.message.ifEmpty { "Request failed (${err.code})" }
            }
            is BridgeError.Protocol -> "Internal error: ${err.message}"
            null -> e.message ?: "Something went wrong"
        }
        error(msg)
    }
}
