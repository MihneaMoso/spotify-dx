package com.spotifydx.app

/**
 * Single session-end orchestrator (§5 contracts). Every logout in the app
 * executes one of these two mechanisms — the per-cause table below is the
 * ONLY place that decides what dies on which cause. Callers know the cause
 * and pick the mechanism; they never hand-roll wipe sequences.
 *
 * Per-cause table (behavior-identical to the pre-extraction call sites):
 * - explicit sign-out, in-flow expiry (`withSessionCheck` definitive or
 *   direct `SessionExpired`) → [full]: memory caches, queue + past-window
 *   disk state, core credentials, mirror. The play log survives (device-
 *   level, reinstall payload — see `PlaybackStore.clearSession`).
 * - watchdog near-expiry with failed refresh → [credentialsOnly]: core
 *   credentials + mirror only (queue/memory survive, so a same-account
 *   relogin resumes playback state). The watchdog keeps its own toast.
 *
 * Both are idempotent (clears on empty stores, logout on a logged-out
 * core, mirror reset to the same snapshot) — concurrent or repeated ends
 * converge to the same state, never corrupt it.
 */
object SessionEnd {
    /**
     * Full end: session-scoped memory caches (no cross-account bleed),
     * persisted queue/past-window (belong to the account), core
     * credentials (authoritative), mirror reset (routes to GATE).
     */
    suspend fun full() {
        // Session-scoped memory caches die with the session (no cross-account
        // bleed); the core clears its own stores in BridgeClient.logout().
        MusicRepository.clear()
        // Persisted queue/past-window belong to the account: wipe them so
        // the next sign-in starts clean. The play log is device-level and
        // survives (see PlaybackStore.clearSession).
        PlaybackStore.clearSession()
        BridgeClient.logout()
        SessionRepository.resetMirror()
    }

    /** Credential-only end: core credentials + mirror reset. */
    suspend fun credentialsOnly() {
        BridgeClient.logout()
        SessionRepository.resetMirror()
    }
}
