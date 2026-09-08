package com.spotifydx.app

import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.asSharedFlow

/**
 * Error-to-toast bus (§10.1 shell, §15 ARCHITECTURE): user-facing errors with
 * timed auto-dismiss (stale timers cannot clear newer errors — the toast view
 * tags each message with a generation) and manual dismiss.
 */
object ToastBus {
    data class Toast(val message: String, val generation: Long)

    private var generation = 0L
    private val _toasts = MutableSharedFlow<Toast>(extraBufferCapacity = 8)
    val toasts: SharedFlow<Toast> = _toasts.asSharedFlow()

    fun error(message: String) {
        generation += 1
        _toasts.tryEmit(Toast(message, generation))
    }

    fun fromBridge(e: Throwable) {
        val msg = when (val err = (e as? BridgeException)?.error) {
            is BridgeError.NotWired -> "Not available yet (${err.phase})"
            is BridgeError.NeedsPage -> "Session expired — signing you back in…"
            is BridgeError.SessionExpired -> "Session expired — please sign in again"
            is BridgeError.RateLimited -> "Spotify's API is temporarily limiting requests — retrying automatically."
            is BridgeError.Core -> err.message.ifEmpty { "Request failed (${err.code})" }
            is BridgeError.Protocol -> "Internal error: ${err.message}"
            null -> e.message ?: "Something went wrong"
        }
        error(msg)
    }
}
