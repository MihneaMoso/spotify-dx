package com.spotifydx.app

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch

/**
 * Authentication state (§5–§6 ARCHITECTURE, §8 migration). Owns the gate
 * boolean: the root switch renders the login gate or the routed shell on this
 * alone. The token itself lives in the core's credential store; Kotlin keeps
 * only the mirror the bridge exposes.
 *
 * Token updates must NOT trigger data re-fetches (screens recover on explicit
 * timers, exactly as today) — repositories read [snapshot] directly instead
 * of collecting it, unless they own the gate.
 */
object SessionRepository {
    data class Snapshot(
        val authenticated: Boolean = false,
        val hasToken: Boolean = false,
        val expiresAtMs: Long = 0,
        /** Account tier for the Phase 5 engine router (auto ⇒ SDK iff premium). */
        val isPremium: Boolean = false,
    )

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val _state = MutableStateFlow(Snapshot())
    val state: StateFlow<Snapshot> = _state.asStateFlow()

    fun snapshot(): Snapshot = _state.value

    fun refresh() {
        scope.launch {
            val json = BridgeClient.sessionStatus().getOrNull() ?: return@launch
            val user = json.optJSONObject("user")
            val next = Snapshot(
                authenticated = json.optBoolean("authenticated", false),
                hasToken = json.optBoolean("has_token", false),
                expiresAtMs = json.optLong("expires_at_ms", 0),
                isPremium = user?.optString("product", "") == "premium",
            )
            // Compare-before-write: touching the store re-renders subscribers.
            if (_state.value != next) _state.value = next
        }
    }

    /** Kotlin login page reports a captured web-player session (§8.4). */
    fun notifyCaptured(accessToken: String, expiresAtMs: Long, userJson: String?) {
        scope.launch {
            val payload = org.json.JSONObject()
                .put("access_token", accessToken)
                .put("expires_at_ms", expiresAtMs)
            if (userJson != null) payload.put("user", org.json.JSONObject(userJson))
            BridgeClient.notifySession(payload.toString()).getOrNull()
            refresh()
        }
    }

    fun logout() {
        scope.launch {
            BridgeClient.logout()
            _state.value = Snapshot()
        }
    }

    /**
     * Silent-restore verifier (Phase 2 gate): when a clock-valid token
     * survived restart, a single low-volume `/v1/me` proves it server-side.
     * Success flips the mirror authenticated (shell, no login page);
     * rejection clears to the gate; anything else stays gated (the login
     * page captures from cookies instead).
     */
    fun verifyAtBoot() {
        scope.launch {
            val s = _state.value
            if (s.authenticated || !s.hasToken) return@launch
            val res = BridgeClient.currentUser()
            if (res.isSuccess) {
                refresh()
            } else {
                val err = (res.exceptionOrNull() as? BridgeException)?.error
                if (err is BridgeError.SessionExpired) {
                    BridgeClient.logout()
                    _state.value = Snapshot()
                }
            }
        }
    }

    /**
     * Expiry watchdog: recovery is timer-driven, never token-reactive
     * (§14.2 — screens must not re-fetch on token updates). When the mirror
     * nears expiry, one shared refresh flight runs; hard failure is a
     * first-class transition back to the gate.
     */
    private var watchdog: Job? = null

    fun startWatchdog() {
        if (watchdog?.isActive == true) return
        watchdog = scope.launch {
            while (isActive) {
                delay(30_000)
                val s = _state.value
                if (!s.authenticated) continue
                val skewMs = 5 * 60_000L
                if (s.expiresAtMs - System.currentTimeMillis() < skewMs) {
                    val res = SessionRefresher.refresh()
                    if (res.isFailure) {
                        // Hard expiry: clear the flag; the gate returns and
                        // the still-valid cookies usually re-authenticate
                        // silently there.
                        BridgeClient.logout()
                        _state.value = Snapshot()
                        ToastBus.fromBridge(
                            res.exceptionOrNull() ?: Exception("session expired"),
                        )
                    } else {
                        refresh()
                    }
                }
            }
        }
    }

    fun onEvent(e: EventBus.AppEvent) {
        if (e is EventBus.AppEvent.Session) refresh()
    }
}
