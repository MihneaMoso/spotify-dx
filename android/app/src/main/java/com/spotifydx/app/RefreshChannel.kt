package com.spotifydx.app

import kotlinx.coroutines.suspendCancellableCoroutine
import kotlin.coroutines.resume

/**
 * Hidden silent-refresh channel (§5.4). Owns everything refresh needs and
 * nothing it doesn't: ensuring the hidden page, one revive flight at a
 * time, outcome classification, parking afterwards. Shares the [SessionPage]
 * (cookies, realized view, IPC, load/error signals) with [LoginPage] by
 * construction — never shows anything, never navigates visibly.
 */
class RefreshChannel(private val page: SessionPage) {
    /**
     * Revive-flight outcome (cold-boot expiry fix): a loaded-but-tokenless
     * page proves the web session itself is dead (only interactive login
     * helps), while a page that never loaded is transient (offline/slow —
     * keep the token, error+retry as today). Conflating them stranded
     * expiry-while-closed boots on a dead retry forever.
     */
    enum class ReviveOutcome { CAPTURED, NO_SESSION, PAGE_DEAD }

    /**
     * Revive the session page and mint a fresh token on demand (§5.4):
     * ensure the page (hidden — cold boots have none yet), navigate,
     * wait for load completion, run the refresh routine, park again
     * afterwards. The outcome distinguishes a dead web session (page
     * rendered, no token → interactive login is the only way out) from a
     * page that never rendered (transient — keep today's error+retry).
     */
    suspend fun reviveAndRefresh(timeoutMs: Long = 15_000): ReviveOutcome {
        val wv = page.ensure()
        return suspendCancellableCoroutine { cont ->
            fun finish(outcome: ReviveOutcome) {
                if (!cont.isActive) return
                page.reviveArmed = false
                page.reviveHook = null
                page.park()
                cont.resume(outcome)
            }
            // Single outcome computation at every exit: a capture wins;
            // otherwise a main-frame error or a never-rendered page is
            // transient, and a rendered-but-tokenless page is a dead web
            // session. (onPageFinished fires for failed loads too, so the
            // error flag — not loaded alone — carries "never rendered".)
            fun compute(): ReviveOutcome = when {
                page.reviveCaptured -> ReviveOutcome.CAPTURED
                page.reviveError || !page.reviveLoaded -> ReviveOutcome.PAGE_DEAD
                else -> ReviveOutcome.NO_SESSION
            }
            page.post {
                page.reviveArmed = true
                page.reviveLoaded = false
                page.reviveError = false
                page.reviveCaptured = false
                page.reviveHook = {
                    wv.evaluateJavascript(CaptureJs.REFRESH, null)
                    // Early-out on actual capture only: a tokenless page at
                    // 9s may still be a slow capture, so NO_SESSION is
                    // declared solely at the timeout exit (same 15s bound
                    // as today's failure declaration).
                    page.postDelayed({
                        if (page.reviveCaptured) finish(ReviveOutcome.CAPTURED)
                    }, 9_000)
                }
                wv.loadUrl(SessionPage.PLAYER_URL)
            }
            page.postDelayed({
                if (cont.isActive) finish(compute())
            }, timeoutMs)
        }
    }
}
