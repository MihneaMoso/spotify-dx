package com.spotifydx.app

import android.view.View
import android.widget.Button
import android.widget.TextView

/**
 * Shared loading/error/empty/content contract (§10.2 migration): every page
 * renders the same four states with identical behavior — skeleton text while
 * loading, inline banner + retry for page-level failures (rate-limit message
 * + automatic retry preserved), and a plain empty note.
 */
sealed interface ScreenState {
    data object Loading : ScreenState
    data class Content(val empty: Boolean = false) : ScreenState
    data class Error(val message: String, val retry: () -> Unit) : ScreenState
}

/** Binds a [ScreenState] to a (message, retry, content) triple of views. */
object UiStates {
    fun bind(
        state: ScreenState,
        content: View,
        message: TextView,
        retry: Button,
        loadingText: String = "Loading…",
    ) {
        when (state) {
            is ScreenState.Loading -> {
                content.visibility = View.GONE
                retry.visibility = View.GONE
                message.visibility = View.VISIBLE
                message.text = loadingText
            }
            is ScreenState.Content -> {
                message.visibility = View.GONE
                retry.visibility = View.GONE
                content.visibility = View.VISIBLE
            }
            is ScreenState.Error -> {
                content.visibility = View.GONE
                message.visibility = View.VISIBLE
                message.text = state.message
                retry.visibility = View.VISIBLE
                retry.setOnClickListener { state.retry() }
            }
        }
    }

    /** Maps a [Result] failure to user copy (rate-limit copy preserved). */
    fun errorCopy(e: Throwable?): String {
        val err = (e as? BridgeException)?.error
        return when (err) {
            is BridgeError.RateLimited ->
                "Spotify's API is temporarily limiting requests — retrying automatically."
            is BridgeError.Core ->
                if (err.code == "RATE_LIMITED" || err.message.contains("rate", ignoreCase = true)) {
                    "Spotify's API is temporarily limiting requests — retrying automatically."
                } else {
                    err.message.ifEmpty { "Couldn't load. Tap retry." }
                }
            is BridgeError.SessionExpired -> "Session expired — please sign in again."
            is BridgeError.NeedsPage -> "Session expired — signing you back in…"
            is BridgeError.NotWired -> "Nothing here yet — this screen wires up in ${err.phase}."
            else -> e?.message ?: "Couldn't load. Tap retry."
        }
    }
}
