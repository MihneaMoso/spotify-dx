package com.spotifydx.app

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Deferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.async
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

/**
 * Token refresh driver (§5.4 ARCHITECTURE, §8 migration). Documented
 * inversion: the session page lives in Kotlin, so refresh initiates here —
 * the core only judges the mirror (`refreshToken` → fresh / `NEEDS_PAGE`).
 *
 * Semantics match the native fan-out: concurrent refresh requests share one
 * flight (a collection of waiters, not a single slot), the wait is bounded
 * (~20s), and a lost answer re-checks the mirror first — a page capture may
 * have landed the fresh token anyway.
 */
object SessionRefresher {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val mutex = Mutex()
    private var inFlight: Deferred<Result<Unit>>? = null

    /** Supplied by MainActivity (the page owner). Null after logout teardown. */
    var pageHost: (() -> LoginWebViewManager?)? = null

    suspend fun refresh(): Result<Unit> {
        val flight = mutex.withLock {
            inFlight?.takeIf { it.isActive } ?: scope.async { runFlight() }.also {
                inFlight = it
            }
        }
        return flight.await()
    }

    private suspend fun runFlight(): Result<Unit> {
        // Fast path: core says the mirror is fresh.
        val probe = BridgeClient.refreshToken()
        if (probe.isSuccess) return Result.success(Unit)

        val err = probe.exceptionOrNull()
        if ((err as? BridgeException)?.error !is BridgeError.NeedsPage) {
            return Result.failure(err ?: Exception("refresh failed"))
        }
        // Slow path: revive the session page, capture, retry. The page posts
        // token_refresh_result through the JS bridge on success.
        val host = pageHost?.invoke() ?: return Result.failure(
            BridgeException(BridgeError.NeedsPage("no session page")),
        )
        val revived = host.reviveAndRefresh()
        if (!revived) return Result.failure(
            BridgeException(BridgeError.NeedsPage("session page revive failed")),
        )
        SessionRepository.refresh()
        val s = SessionRepository.snapshot()
        return if (s.authenticated) {
            Result.success(Unit)
        } else {
            // Lost answer? Re-check the mirror before declaring failure.
            val retry = BridgeClient.refreshToken()
            if (retry.isSuccess) Result.success(Unit)
            else Result.failure(retry.exceptionOrNull() ?: Exception("refresh failed"))
        }
    }
}
