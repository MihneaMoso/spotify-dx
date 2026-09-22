package com.spotifydx.app

/**
 * Pure session/error policy (§5 contracts). Every session decision in the
 * app funnels through these functions so the rules live in ONE readable
 * place instead of scattered pattern-matches across repositories,
 * ViewModels, and the watchdog.
 *
 * Pure Kotlin only (no Android imports): decisions take the typed
 * [BridgeError] in and return a verdict out. Execution (refresh flights,
 * logout side effects, navigation) stays with the callers — this file
 * never launches coroutines, touches the bridge, or navigates.
 *
 * Decision table (behavior-identical to the pre-extraction code):
 * - data fetch fails `NeedsPage` → [DataAction.HealThenRetry] (silent
 *   refresh once, then retry the fetch — never a logout; a logout here
 *   wiped disk state on transient boot races and poisoned every retry).
 * - any other data error → [DataAction.ReturnAsIs] (page-local error;
 *   `SessionExpired` additionally ends the session via [requiresLogout]).
 * - heal fails definitively (web session dead) → surface the heal error
 *   ([healErrorToSurface]), which requires logout → GATE login. Heal
 *   fails transiently → keep the ORIGINAL `NeedsPage` (today's
 *   error+retry; the token is kept).
 * - [requiresLogout] is true for `SessionExpired` ONLY. `NeedsPage` never
 *   reaches it (the heal branch always returns first) — a `NeedsPage`
 *   must never log out.
 */
object SessionPolicy {
    /** What a failed data fetch means for the session. */
    sealed interface DataAction {
        /** Silent-refresh once, retry the fetch on success. */
        data object HealThenRetry : DataAction

        /** Surface the error page-locally (logout iff [requiresLogout]). */
        data object ReturnAsIs : DataAction
    }

    fun onDataError(error: BridgeError?): DataAction =
        if (error is BridgeError.NeedsPage) DataAction.HealThenRetry
        else DataAction.ReturnAsIs

    /**
     * Which error a failed heal surfaces. Definitive heal failure (the
     * refresher proved the web session dead) REPLACES the original
     * `NeedsPage`: collapsing back into it stranded expiry boots on a
     * retry that replays the same instant failure forever. Transient heal
     * failure keeps the original (token kept, error+retry as today).
     */
    fun healErrorToSurface(
        original: BridgeError.NeedsPage,
        healError: BridgeError?,
    ): BridgeError =
        if (healError is BridgeError.SessionExpired) healError else original

    /**
     * Whether surfacing this error must end the session (logout → GATE
     * login). True for `SessionExpired` only.
     */
    fun requiresLogout(error: BridgeError?): Boolean =
        error is BridgeError.SessionExpired

    /** Rate-limit detection shared by the screen ViewModels (§7.5). */
    fun isRateLimited(e: Throwable?): Boolean {
        val err = (e as? BridgeException)?.error
        return err is BridgeError.RateLimited ||
            (err is BridgeError.Core && err.message.contains("rate", ignoreCase = true))
    }
}
