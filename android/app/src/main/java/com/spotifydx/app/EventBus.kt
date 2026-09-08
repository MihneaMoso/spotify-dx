package com.spotifydx.app

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.launch

/**
 * Core→Kotlin events (§6.3) delivered as a polled future: a background loop
 * drains the bridge queue ([CoreBridge.pollEvents]) and replays items onto a
 * [SharedFlow]. Screens collect the flow on the main thread — every listener
 * marshals to main before touching interface state (§12.2).
 */
object EventBus {
    data class CoreEvent(val kind: String, val payloadJson: String)

    sealed interface AppEvent {
        /** Session mirror flipped (authenticated flag / restore / logout). */
        data class Session(val authenticated: Boolean) : AppEvent
        /** Update check finished / download staged. Payload is raw JSON. */
        data class Update(val payloadJson: String) : AppEvent
        /** Filter drop burst for the privacy panel. */
        data class Filter(val payloadJson: String) : AppEvent
    }

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private val _events = MutableSharedFlow<AppEvent>(extraBufferCapacity = 64)
    val events: SharedFlow<AppEvent> = _events.asSharedFlow()

    private var started = false

    /** Idempotent; safe to call from every Activity recreation. */
    fun start() {
        if (started) return
        started = true
        scope.launch {
            while (true) {
                runCatching {
                    BridgeClient.pollEvents().getOrNull().orEmpty()
                }.getOrDefault(emptyList()).forEach { dispatch(it) }
                // Same event-driven cadence the core uses: the queue is empty
                // almost always, so a slow poll here costs nothing.
                delay(2_000)
            }
        }
    }

    private fun dispatch(e: CoreEvent) {
        val app: AppEvent = when (e.kind) {
            "session" -> AppEvent.Session(
                e.payloadJson.contains("\"authenticated\":true"),
            )
            "update" -> AppEvent.Update(e.payloadJson)
            "filter" -> AppEvent.Filter(e.payloadJson)
            else -> return
        }
        _events.tryEmit(app)
    }
}
