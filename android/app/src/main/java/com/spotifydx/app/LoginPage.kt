package com.spotifydx.app

/**
 * Visible interactive sign-in flow (§8 migration). Owns everything the user
 * SEES: showing the fullscreen login page for a sign-in URL, hiding it on
 * capture, and the full logout teardown (cookie pages + credentials +
 * mirror). Silent refresh lives in [RefreshChannel]; both share one
 * [SessionPage] (cookies, realized view, IPC) by construction.
 */
class LoginPage(private val page: SessionPage) {
    /** Build + show the fullscreen login page. Idempotent (re-entry guard). */
    fun show(url: String = SessionPage.SIGN_IN_URL) {
        page.show(url)
    }

    fun hide() {
        page.hide()
    }

    /**
     * Logout teardown (§8.6): clear credentials (bridge), reset state, tear
     * down cookie pages so the next login starts clean. Cookie clearing is
     * best-effort; credential clearing is authoritative.
     */
    fun destroyForLogout() {
        page.resetCapture()
        page.teardownView()
        page.clearCookies()
        SessionRepository.logout()
    }
}
