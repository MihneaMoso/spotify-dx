package com.spotifydx.app

/**
 * Pure gate-routing table (§4.2 contracts). The shell-first rule — land on
 * HOME, route to GATE if and only if the core definitively reports
 * signed-out — lived in three near-identical inline conditions (settled
 * collector, fresh-device fast lane, wedged-core backstop). All three now
 * execute this one decision function; the "when may we route" signal
 * differs per site, the destination rule never does.
 *
 * Pure Kotlin only (no Android imports): inputs are snapshot/mirror
 * primitives, output is a destination decision. Navigation (`go(...)`)
 * and side effects (overlay hide, store reads) stay with the callers.
 *
 * Destination rule (behavior-identical to the pre-extraction code):
 * - authenticated on the gate → GO_HOME (leave a stale gate behind).
 * - authenticated elsewhere → STAY (session proved; no flash, no jump).
 * - unauthenticated → GO_GATE only when routing is allowed AND no token
 *   exists at all (a present-but-unverified token stays shell-first while
 *   verification/capture proves it) AND not already there; else STAY.
 */
object GatePolicy {
    enum class Decision { GO_HOME, GO_GATE, STAY }

    fun decide(
        authenticated: Boolean,
        hasToken: Boolean,
        mayRoute: Boolean,
        isGate: Boolean,
    ): Decision = when {
        authenticated && isGate -> Decision.GO_HOME
        authenticated -> Decision.STAY
        mayRoute && !hasToken && !isGate -> Decision.GO_GATE
        else -> Decision.STAY
    }
}
