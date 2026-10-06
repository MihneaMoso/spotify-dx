package com.spotifydx.app;

/**
 * Regression tests for pure-Kotlin policy + session-capture logic. Plain
 * JVM, zero dependencies: no JUnit on this machine and the Gradle build
 * runs offline, so assertions are hand-rolled {@code check()} calls driven
 * from {@code main()} (same harness contract as RangeServeTest).
 *
 * Run (from repo root):
 *   cd android && ./gradlew assembleDebug --offline   # rebuild classes
 *   STDLIB=$(find ~/.gradle/caches -name "kotlin-stdlib-2*.jar" | head -1)
 *   CP=app/build/tmp/kotlin-classes/debug:$STDLIB
 *   javac -cp "$CP" -d /tmp/rstest app/src/test/java/com/spotifydx/app/LogicRegressionTest.java
 *   java -cp "/tmp/rstest:$CP" com.spotifydx.app.LogicRegressionTest
 *
 * Every case encodes a real session/playback failure: the expiry retry
 * hell (heal routing), the gate flash rules, the TOTP capture contract,
 * and the capture-JS narrowing.
 */
public class LogicRegressionTest {
    static int passed = 0;

    static void check(boolean cond, String name) {
        if (!cond) throw new AssertionError("FAILED: " + name);
        passed++;
    }

    public static void main(String[] args) {
        sessionPolicy();
        gatePolicy();
        totpVectors();
        captureJsNarrowing();
        System.out.println("LogicRegressionTest: " + passed + " checks passed");
    }

    // -- SessionPolicy: the expiry-heal routing table -------------------------
    static void sessionPolicy() {
        // NeedsPage heals silently once, then retries — never a logout.
        check(
            SessionPolicy.INSTANCE.onDataError(new BridgeError.NeedsPage("stale"))
                instanceof SessionPolicy.DataAction.HealThenRetry,
            "needspage heals then retries");
        // Any other data error surfaces page-locally.
        check(
            SessionPolicy.INSTANCE.onDataError(
                new BridgeError.Core("NET", "down"))
                instanceof SessionPolicy.DataAction.ReturnAsIs,
            "core error returns as-is");
        check(
            SessionPolicy.INSTANCE.onDataError(null)
                instanceof SessionPolicy.DataAction.ReturnAsIs,
            "null error returns as-is");
        // Definitive heal failure (dead web session) surfaces the heal
        // error and requires logout → GATE login owns the UX.
        BridgeError.NeedsPage original = new BridgeError.NeedsPage("stale");
        BridgeError expired = new BridgeError.SessionExpired("dead");
        check(
            SessionPolicy.INSTANCE.healErrorToSurface(original, expired) == expired,
            "definitive heal failure replaces original");
        // Transient heal failure keeps the original NeedsPage (token kept,
        // error + retry as today).
        BridgeError transientHeal = new BridgeError.NeedsPage("unreachable");
        check(
            SessionPolicy.INSTANCE.healErrorToSurface(original, transientHeal) == original,
            "transient heal failure keeps original");
        // Only SessionExpired ends the session; NeedsPage never logs out.
        check(SessionPolicy.INSTANCE.requiresLogout(expired), "expired requires logout");
        check(!SessionPolicy.INSTANCE.requiresLogout(original), "needspage never logs out");
        check(!SessionPolicy.INSTANCE.requiresLogout(null), "null never logs out");
        // Rate-limit detection (banner + timed retry contract).
        check(
            SessionPolicy.INSTANCE.isRateLimited(
                new BridgeException(new BridgeError.RateLimited("slow"))),
            "ratelimited typed");
        check(
            SessionPolicy.INSTANCE.isRateLimited(
                new BridgeException(new BridgeError.Core("X", "API rate limit hit"))),
            "rate substring in core message");
        check(
            !SessionPolicy.INSTANCE.isRateLimited(
                new BridgeException(new BridgeError.Core("NET", "down"))),
            "plain net is not rate limited");
    }

    // -- GatePolicy: shell-first routing table --------------------------------
    static void gatePolicy() {
        // Authenticated on the gate → home (leave a stale gate behind).
        check(
            GatePolicy.INSTANCE.decide(true, false, true, true)
                == GatePolicy.Decision.GO_HOME,
            "authed on gate goes home");
        // Authenticated elsewhere → stay (no flash, no jump).
        check(
            GatePolicy.INSTANCE.decide(true, true, true, false)
                == GatePolicy.Decision.STAY,
            "authed elsewhere stays");
        // Unauthenticated, tokenless, routing allowed, not on gate → gate.
        check(
            GatePolicy.INSTANCE.decide(false, false, true, false)
                == GatePolicy.Decision.GO_GATE,
            "tokenless boots take the gate lane");
        // A present-but-unverified token stays shell-first while capture
        // proves it — never a gate flash for valid sessions.
        check(
            GatePolicy.INSTANCE.decide(false, true, true, false)
                == GatePolicy.Decision.STAY,
            "unverified token stays shell-first");
        // Routing not allowed (core still booting) → stay put.
        check(
            GatePolicy.INSTANCE.decide(false, false, false, false)
                == GatePolicy.Decision.STAY,
            "unsettled core stays put");
        // Already on gate → stay (no redundant navigation).
        check(
            GatePolicy.INSTANCE.decide(false, false, true, true)
                == GatePolicy.Decision.STAY,
            "already on gate stays");
    }

    // -- SessionTokenFetch.totp: RFC 6238 vectors --------------------------------
    // Independent Python reference (HMAC-SHA1, key = ASCII bytes of the
    // decimal TOTP_KEY, 30s period, dynamic truncation mod 1e6):
    //   ms=0 → 204513, ms=30000 → 332823, ms=1791100000000 → 861936.
    // A wrong key digest or SHA-1 constant silently mints rejected TOTPs,
    // so this pins the implementation, not just the shape.
    static void totpVectors() {
        check(SessionTokenFetch.INSTANCE.totp(0L).equals("204513"), "totp@0");
        check(SessionTokenFetch.INSTANCE.totp(1000L).equals("204513"), "totp same window");
        check(SessionTokenFetch.INSTANCE.totp(29999L).equals("204513"), "totp window edge");
        check(SessionTokenFetch.INSTANCE.totp(30000L).equals("332823"), "totp next window");
        check(
            SessionTokenFetch.INSTANCE.totp(1791100000000L).equals("861936"),
            "totp realistic timestamp");
    }

    // -- CaptureJs.POLL_JS: narrowed fetch hook ----------------------------------
    // The hook used to match any URL containing "token" (CSRF/clientToken
    // traffic included) and clone-read every such body. Narrowed to the
    // two endpoints returning {accessToken}; the Kotlin copy must stay
    // verbatim with the native core bundle.
    static void captureJsNarrowing() {
        String js = CaptureJs.POLL_JS;
        check(js.contains("/api/token"), "hook watches api/token");
        check(js.contains("get_access_token"), "hook watches legacy endpoint");
        check(!js.contains("indexOf('clientToken')"), "clientToken no longer hooked");
        check(!js.contains("indexOf('token')"), "bare token no longer hooked");
        check(js.contains(SessionTokenFetch.TOTP_KEY), "totp key matches fetcher");
    }
}
