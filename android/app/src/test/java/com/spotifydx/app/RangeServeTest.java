package com.spotifydx.app;

import java.util.Map;

/**
 * Regression tests for {@link RangeServe} (the local Range-proxy serving
 * logic). Plain JVM, zero dependencies: no JUnit on this machine and the
 * Gradle build runs offline, so assertions are hand-rolled {@code check()}
 * calls driven from {@code main()}.
 *
 * Run (from repo root):
 *   cd android && ./gradlew assembleDebug --offline   # rebuild classes
 *   STDLIB=$(find ~/.gradle/caches -name "kotlin-stdlib-2*.jar" | head -1)
 *   CP=app/build/tmp/kotlin-classes/debug:$STDLIB
 *   javac -cp "$CP" -d /tmp/rstest app/src/test/java/com/spotifydx/app/RangeServeTest.java
 *   java -cp "/tmp/rstest:$CP" com.spotifydx.app.RangeServeTest
 *
 * Every case encodes a real playback failure (mostly the opus/WebM
 * outage: tail seeks for Cues deleted the valid prefix, teed tail bytes
 * at file offset 0, and lied about spans — while MP4/AAC sailed through
 * with front-loaded headers).
 */
public class RangeServeTest {
    static int passed = 0;

    static void check(boolean cond, String name) {
        if (!cond) throw new AssertionError("FAILED: " + name);
        passed++;
    }

    static void eq(Object a, Object b, String name) {
        if (a == null ? b != null : !a.equals(b)) {
            throw new AssertionError("FAILED: " + name + " expected=" + b + " actual=" + a);
        }
        passed++;
    }

    public static void main(String[] args) throws Exception {
        parseRangeTests();
        decideTests();
        gapRegressionTests();
        headTests();
        mimeTests();
        redirectTests();
        teeGateTests();
        seqFillTests();
        catchUpSpanTests();
        System.out.println("RangeServeTest: " + passed + " checks passed");
    }

    // -- Range parsing -------------------------------------------------------
    static void parseRangeTests() {
        RangeServe.RangeReq r = RangeServe.INSTANCE.parseRange("bytes=0-");
        eq(r.getStart(), 0L, "open range start");
        eq(r.getEnd(), null, "open range end");

        r = RangeServe.INSTANCE.parseRange("bytes=100-200");
        eq(r.getStart(), 100L, "closed start");
        eq(r.getEnd(), 200L, "closed end");

        r = RangeServe.INSTANCE.parseRange("bytes=-500");
        eq(r.getSuffix(), 500L, "suffix parsed");

        r = RangeServe.INSTANCE.parseRange("banana");
        check(r.isFull(), "garbage is full request");

        r = RangeServe.INSTANCE.parseRange(null);
        check(r.isFull(), "null is full request");

        r = RangeServe.INSTANCE.parseRange("bytes=-0");
        check(r.isFull(), "zero suffix is full request");

        r = RangeServe.INSTANCE.parseRange("bytes=5-3");
        eq(r.getStart(), 5L, "inverted span still parses (decide 416s it)");
    }

    // -- Serving decisions ----------------------------------------------------
    static void decideTests() {
        // Fresh full fetch.
        RangeServe.Decision d = RangeServe.INSTANCE.decide(
            RangeServe.INSTANCE.parseRange(null), 0, 1000, true);
        check(d instanceof RangeServe.Decision.FetchSpan, "fresh full fetches");
        RangeServe.Decision.FetchSpan f = (RangeServe.Decision.FetchSpan) d;
        eq(f.getRangeHeader(), null, "fresh fetch has no Range");
        check(f.getStore() && !f.getDeleteFirst(), "fresh fetch stores clean");

        // Complete file serves whole.
        d = RangeServe.INSTANCE.decide(
            RangeServe.INSTANCE.parseRange(null), 1000, 1000, true);
        check(d instanceof RangeServe.Decision.ServeSpan, "complete file serves");
        RangeServe.Decision.ServeSpan s = (RangeServe.Decision.ServeSpan) d;
        eq(s.getStart(), 0L, "complete start");
        eq(s.getEnd(), 999L, "complete end");

        // Partial prefix + known total + upstream: RESTART-FILL (never a
        // closed stub — stubs EOS at the cached end = fixed-timestamp skip).
        d = RangeServe.INSTANCE.decide(
            RangeServe.INSTANCE.parseRange(null), 400, 1000, true);
        check(d instanceof RangeServe.Decision.FetchSpan, "partial restarts fill");
        f = (RangeServe.Decision.FetchSpan) d;
        eq(f.getRangeHeader(), null, "restart is a full fetch");
        check(f.getStore() && f.getDeleteFirst(), "restart stores with delete");

        // Partial prefix WITHOUT upstream: the stub is all there is.
        d = RangeServe.INSTANCE.decide(
            RangeServe.INSTANCE.parseRange(null), 400, 1000, false);
        s = (RangeServe.Decision.ServeSpan) d;
        eq(s.getEnd(), 399L, "offline stub end");

        // In-span range from cache.
        d = RangeServe.INSTANCE.decide(
            RangeServe.INSTANCE.parseRange("bytes=100-200"), 500, 1000, true);
        s = (RangeServe.Decision.ServeSpan) d;
        eq(s.getStart(), 100L, "cached span start");
        eq(s.getEnd(), 200L, "cached span end");

        // Beyond total → 416 with total.
        d = RangeServe.INSTANCE.decide(
            RangeServe.INSTANCE.parseRange("bytes=5000-"), 500, 1000, true);
        check(d instanceof RangeServe.Decision.Fail, "beyond total 416s");
        eq(((RangeServe.Decision.Fail) d).getCode(), 416, "416 code");
        eq(((RangeServe.Decision.Fail) d).getTotal(), 1000L, "416 carries total");

        // Inverted span → 416.
        d = RangeServe.INSTANCE.decide(
            RangeServe.INSTANCE.parseRange("bytes=5-3"), 500, 1000, true);
        check(d instanceof RangeServe.Decision.Fail, "inverted span 416s");

        // Exact append tees.
        d = RangeServe.INSTANCE.decide(
            RangeServe.INSTANCE.parseRange("bytes=500-"), 500, 1000, true);
        f = (RangeServe.Decision.FetchSpan) d;
        eq(f.getRangeHeader(), "bytes=500-", "append forwards exact range");
        check(f.getStore() && !f.getDeleteFirst(), "append stores without delete");

        // Restart replaces stale partial.
        d = RangeServe.INSTANCE.decide(
            RangeServe.INSTANCE.parseRange("bytes=0-"), 500, 1000, true);
        f = (RangeServe.Decision.FetchSpan) d;
        check(f.getStore() && f.getDeleteFirst(), "restart stores with delete");

        // Suffix with known total resolves to the tail span.
        d = RangeServe.INSTANCE.decide(
            RangeServe.INSTANCE.parseRange("bytes=-500"), 1000, 1000, true);
        s = (RangeServe.Decision.ServeSpan) d;
        eq(s.getStart(), 500L, "suffix start");
        eq(s.getEnd(), 999L, "suffix end");

        // Suffix, unknown total, upstream → verbatim forward, no store.
        d = RangeServe.INSTANCE.decide(
            RangeServe.INSTANCE.parseRange("bytes=-500"), 100, -1, true);
        f = (RangeServe.Decision.FetchSpan) d;
        eq(f.getRangeHeader(), "bytes=-500", "suffix forwarded verbatim");
        check(!f.getStore(), "suffix fetch never stores");

        // Suffix, unknown total, no upstream → 416 with unknown total.
        d = RangeServe.INSTANCE.decide(
            RangeServe.INSTANCE.parseRange("bytes=-500"), 0, -1, false);
        check(d instanceof RangeServe.Decision.Fail, "suffix without upstream 416s");

        // No upstream, empty cache → 502.
        d = RangeServe.INSTANCE.decide(
            RangeServe.INSTANCE.parseRange(null), 0, -1, false);
        eq(((RangeServe.Decision.Fail) d).getCode(), 502, "empty miss 502s");

        // No upstream, partial cache → clamped serve.
        d = RangeServe.INSTANCE.decide(
            RangeServe.INSTANCE.parseRange("bytes=100-900"), 500, 1000, false);
        s = (RangeServe.Decision.ServeSpan) d;
        eq(s.getEnd(), 499L, "clamped to cached bytes");
    }

    // -- The opus outage, encoded ------------------------------------------------
    // WebM Cues live at the file tail: the player seeks beyond the cached
    // prefix. The old code DELETED the valid partial and teed tail bytes at
    // file offset 0 (corrupt cache + corrupt stream with lying offsets).
    static void gapRegressionTests() {
        // Tail seek past the prefix: pass-through, file untouched.
        RangeServe.Decision d = RangeServe.INSTANCE.decide(
            RangeServe.INSTANCE.parseRange("bytes=9000-"), 400, 10000, true);
        check(d instanceof RangeServe.Decision.FetchSpan, "gap fetches upstream");
        RangeServe.Decision.FetchSpan f = (RangeServe.Decision.FetchSpan) d;
        eq(f.getRangeHeader(), "bytes=9000-", "gap forwards the requested span");
        check(!f.getStore(), "gap is never stored");
        check(!f.getDeleteFirst(), "gap never deletes the prefix");

        // Bounded gap keeps its end.
        d = RangeServe.INSTANCE.decide(
            RangeServe.INSTANCE.parseRange("bytes=9000-9500"), 400, 10000, true);
        f = (RangeServe.Decision.FetchSpan) d;
        eq(f.getRangeHeader(), "bytes=9000-9500", "bounded gap keeps end");

        // Straddle (start cached, end beyond) also never writes.
        d = RangeServe.INSTANCE.decide(
            RangeServe.INSTANCE.parseRange("bytes=300-900"), 400, 1000, true);
        f = (RangeServe.Decision.FetchSpan) d;
        check(!f.getStore() && !f.getDeleteFirst(), "straddle never writes");

        // Unknown total: same rule.
        d = RangeServe.INSTANCE.decide(
            RangeServe.INSTANCE.parseRange("bytes=9000-"), 400, -1, true);
        f = (RangeServe.Decision.FetchSpan) d;
        check(!f.getStore() && !f.getDeleteFirst(), "gap never writes (unknown total)");
    }

    // -- Response heads -----------------------------------------------------------
    static void headTests() {
        // Complete full file → 200, exact length, NO Content-Range.
        RangeServe.Head h = RangeServe.INSTANCE.fileHead(0, 999, 1000, "audio/mp4", true);
        eq(h.getCode(), 200, "full file 200s");
        eq(h.getHeaders().get("Content-Length"), "1000", "full file length");
        check(!h.getHeaders().containsKey("Content-Range"), "full file has no range");

        // Partial prefix stub: 206 with TRUTHFUL end/length (the old code
        // claimed the full total while sending `have` bytes → player stall).
        h = RangeServe.INSTANCE.fileHead(0, 399, 1000, "audio/mp4", true);
        eq(h.getCode(), 206, "partial prefix 206s");
        eq(h.getHeaders().get("Content-Range"), "bytes 0-399/1000", "truthful range");
        eq(h.getHeaders().get("Content-Length"), "400", "length equals bytes sent");

        // Unknown total → close-delimited 200.
        h = RangeServe.INSTANCE.fileHead(0, 399, -1, "audio/mp4", true);
        eq(h.getCode(), 200, "unknown total 200s");
        check(!h.getHeaders().containsKey("Content-Length"), "unknown total has no length");

        // Live full fills go close-delimited (no phantom EOS on stalls).
        h = RangeServe.INSTANCE.liveFullHead("audio/webm");
        eq(h.getCode(), 200, "live full 200s");
        check(!h.getHeaders().containsKey("Content-Length"), "live full has no length");
        check(!h.getHeaders().containsKey("Content-Range"), "live full has no range");
        eq(h.getHeaders().get("Content-Type"), "audio/webm", "live full keeps MIME");

        // Upstream 206 with matching span relays verbatim.
        h = RangeServe.INSTANCE.fetchHead(5000L, 206, "bytes 5000-9999/10000", null, "audio/webm");
        eq(h.getCode(), 206, "relay 206s");
        eq(h.getHeaders().get("Content-Range"), "bytes 5000-9999/10000", "relay range verbatim");
        eq(h.getHeaders().get("Content-Length"), "5000", "relay length from span");

        // Upstream 206 with a MISMATCHED span relays nothing invented.
        h = RangeServe.INSTANCE.fetchHead(9000L, 206, "bytes 0-4999/10000", null, "audio/webm");
        check(!h.getHeaders().containsKey("Content-Range"), "mismatch relays no range");
        check(!h.getHeaders().containsKey("Content-Length"), "mismatch relays no length");

        // Upstream 200 with length.
        h = RangeServe.INSTANCE.fetchHead(null, 200, null, "12345", "audio/webm");
        eq(h.getCode(), 200, "upstream 200 stays 200");
        eq(h.getHeaders().get("Content-Length"), "12345", "upstream length kept");

        // 416 carries the mandatory range (or */* unknown).
        h = RangeServe.INSTANCE.unsatisfiableHead(1000);
        eq(h.getHeaders().get("Content-Range"), "bytes */1000", "416 carries total");
        h = RangeServe.INSTANCE.unsatisfiableHead(-1);
        eq(h.getHeaders().get("Content-Range"), "bytes */*", "416 unknown total");

        // Content-Range parser.
        kotlin.Triple<Long, Long, Long> t =
            RangeServe.INSTANCE.parseContentRange("bytes 5000-9999/10000");
        eq(t.getFirst(), 5000L, "CR start");
        eq(t.getSecond(), 9999L, "CR end");
        eq(t.getThird(), 10000L, "CR total");
        check(RangeServe.INSTANCE.parseContentRange("garbage") == null, "CR garbage nulls");
    }

    // -- MIME types ------------------------------------------------------------------
    static void mimeTests() {
        eq(RangeServe.INSTANCE.mimeForFormat("opus"), "audio/webm", "opus mime");
        eq(RangeServe.INSTANCE.mimeForFormat("mp4"), "audio/mp4", "mp4 mime");
        eq(RangeServe.INSTANCE.mimeForFormat("m4a"), "audio/mp4", "m4a mime");
        eq(RangeServe.INSTANCE.mimeForFormat("aac"), "audio/mp4", "aac mime");
        eq(RangeServe.INSTANCE.mimeForFormat("mp3"), "audio/mpeg", "mp3 mime");
        eq(RangeServe.INSTANCE.mimeForFormat("flac"), "audio/flac", "flac mime");
        eq(RangeServe.INSTANCE.mimeForFormat("ogg"), "audio/ogg", "ogg mime");
        eq(RangeServe.INSTANCE.mimeForFormat("mystery"), "application/octet-stream", "unknown mime");
        eq(RangeServe.INSTANCE.mimeForFormat(null), "application/octet-stream", "null mime");
        // Map is exposed for parity checks.
        for (Map.Entry<String, String> e : Map.of(
            "opus", "audio/webm", "mp3", "audio/mpeg", "flac", "audio/flac").entrySet()) {
            eq(RangeServe.INSTANCE.mimeForFormat(e.getKey()), e.getValue(), "mime " + e.getKey());
        }
    }

    // -- Redirect policy ---------------------------------------------------------------
    // The proxy follows redirects manually so Range survives cross-host
    // hops (HttpURLConnection drops request headers there, silently
    // turning span fetches into full bodies).
    static void redirectTests() {
        eq(RangeServe.INSTANCE.resolveRedirect(
            "https://a.test/x", "https://b.test/y"), "https://b.test/y", "absolute https");
        eq(RangeServe.INSTANCE.resolveRedirect(
            "https://a.test/x", "http://b.test/y"), "http://b.test/y", "absolute http");
        eq(RangeServe.INSTANCE.resolveRedirect(
            "https://host.test/a/b", "/path/file"), "https://host.test/path/file", "absolute path");
        eq(RangeServe.INSTANCE.resolveRedirect(
            "https://host.test/a/b", "file"), "https://host.test/a/file", "relative file");
        eq(RangeServe.INSTANCE.resolveRedirect(
            "https://host.test/a/b?z=0", "?x=1"), "https://host.test/a/b?x=1", "query-only");
        check(RangeServe.INSTANCE.resolveRedirect(
            "https://host.test/a", "rtsp://host.test/x") == null, "rtsp refused");
        check(RangeServe.INSTANCE.resolveRedirect(
            "https://host.test/a", "file:///etc/passwd") == null, "file refused");
        check(RangeServe.INSTANCE.resolveRedirect(
            "https://host.test/a", "javascript:alert(1)") == null, "javascript refused");
        check(RangeServe.INSTANCE.resolveRedirect(
            "https://host.test/a", "ftp://host.test/x") == null, "ftp refused");
    }

    // -- Tee registration ------------------------------------------------------------------
    // One tee session per key: concurrent fills collapse to a single
    // writer + pass-throughs (never two writers, never torn prefixes).
    static void teeGateTests() throws Exception {
        check(TeeGate.INSTANCE.tryAcquire("k1"), "first acquire wins");
        check(!TeeGate.INSTANCE.tryAcquire("k1"), "second acquire loses");
        check(TeeGate.INSTANCE.tryAcquire("k2"), "other keys independent");
        TeeGate.INSTANCE.release("k1");
        TeeGate.INSTANCE.release("k2");
        check(TeeGate.INSTANCE.tryAcquire("k1"), "reacquire after release");
        TeeGate.INSTANCE.release("k1");

        // Threaded: exactly one winner among racers (the prepare-starvation fix).
        final int threads = 16;
        final java.util.concurrent.atomic.AtomicInteger wins =
            new java.util.concurrent.atomic.AtomicInteger(0);
        final java.util.concurrent.CountDownLatch start =
            new java.util.concurrent.CountDownLatch(1);
        Thread[] ts = new Thread[threads];
        for (int i = 0; i < threads; i++) {
            ts[i] = new Thread(() -> {
                try { start.await(); } catch (InterruptedException e) { return; }
                if (TeeGate.INSTANCE.tryAcquire("race")) wins.incrementAndGet();
            });
            ts[i].start();
        }
        start.countDown();
        for (Thread t : ts) t.join(5000);
        eq(wins.get(), 1, "exactly one tee winner");
        TeeGate.INSTANCE.release("race");
        check(TeeGate.INSTANCE.tryAcquire("race"), "slot freed after release");
        TeeGate.INSTANCE.release("race");
    }

    // -- Sequential catch-up vs throttled upstreams --------------------------------------------
    // Fake upstream with full gir=yes emulation: full GETs (no Range),
    // open ranges, oversized windows (>512KB), suffixes, and any
    // non-sequential start 403; sequential bounded windows 206 (cursor
    // tracked globally, like the txp transfer state). Backed by com.sun
    // HttpServer (JDK built-in, no dependencies).
    static final long FAKE_WINDOW_MAX = 512L * 1024;
    static byte[] serverContent;
    static java.util.concurrent.atomic.AtomicLong serverCursor;
    static com.sun.net.httpserver.HttpServer server;
    static String serverUrl;

    static void startSeqServer() throws Exception {
        serverContent = new byte[400 * 1024];
        for (int i = 0; i < serverContent.length; i++) {
            serverContent[i] = (byte) (i * 31 + 7);
        }
        serverCursor = new java.util.concurrent.atomic.AtomicLong(0);
        server = com.sun.net.httpserver.HttpServer.create(new java.net.InetSocketAddress("127.0.0.1", 0), 0);
        server.createContext("/s", ex -> {
            try {
                String range = ex.getRequestHeaders().getFirst("Range");
                long total = serverContent.length;
                // Full GET without Range: 403 (gir behavior).
                if (range == null) {
                    ex.sendResponseHeaders(403, -1);
                    return;
                }
                String spec = range.trim();
                if (!spec.startsWith("bytes=")) {
                    ex.sendResponseHeaders(403, -1);
                    return;
                }
                spec = spec.substring("bytes=".length());
                // Open ranges and suffixes: 403 (non-sequential by shape).
                if (!spec.contains("-") || spec.startsWith("-")
                    || spec.endsWith("-")) {
                    ex.sendResponseHeaders(403, -1);
                    return;
                }
                String[] ab = spec.split("-", 2);
                long start = Long.parseLong(ab[0]);
                long end = Math.min(Long.parseLong(ab[1]), total - 1);
                // Oversized windows: 403 (measured: 256–512KB pass, 1MB fails).
                if (end - start + 1 > FAKE_WINDOW_MAX) {
                    ex.sendResponseHeaders(403, -1);
                    return;
                }
                // Strictly sequential (gir behavior): any other start 403s.
                if (start != serverCursor.get()) {
                    ex.sendResponseHeaders(403, -1);
                    return;
                }
                int len = (int) (end - start + 1);
                ex.getResponseHeaders().set("Content-Type", "audio/webm");
                ex.getResponseHeaders().set("Accept-Ranges", "bytes");
                ex.getResponseHeaders().set("Content-Range",
                    "bytes " + start + "-" + end + "/" + total);
                ex.sendResponseHeaders(206, len);
                try (java.io.OutputStream os = ex.getResponseBody()) {
                    os.write(serverContent, (int) start, len);
                }
                serverCursor.set(end + 1);
            } catch (Exception e) {
                try { ex.sendResponseHeaders(502, -1); } catch (Exception ignored) {}
            } finally {
                ex.close();
            }
        });
        server.setExecutor(java.util.concurrent.Executors.newCachedThreadPool());
        server.start();
        serverUrl = "http://127.0.0.1:" + server.getAddress().getPort() + "/s";
    }

    static void stopSeqServer() {
        if (server != null) server.stop(0);
    }

    static SeqFill.Opener serverOpener() {
        return (url, range) -> {
            try {
                java.net.HttpURLConnection c =
                    (java.net.HttpURLConnection) new java.net.URL(url).openConnection();
                c.setConnectTimeout(5000);
                c.setReadTimeout(15000);
                if (range != null) c.setRequestProperty("Range", range);
                c.setInstanceFollowRedirects(false);
                return c;
            } catch (Exception e) {
                return null;
            }
        };
    }

    static java.io.File tempFile(byte[] prefix) throws Exception {
        java.io.File f = java.io.File.createTempFile("seqfill", ".audio");
        f.deleteOnExit();
        if (prefix.length > 0) java.nio.file.Files.write(f.toPath(), prefix);
        return f;
    }

    static byte[] slice(byte[] a, int from, int to) {
        return java.util.Arrays.copyOfRange(a, from, to);
    }

    static void seqFillTests() throws Exception {
        startSeqServer();
        try {
            // 1. Gap recovery end to end: the prefix must come THROUGH
            // the server (its cursor tracks consumption, like gir txp).
            java.io.File f = tempFile(new byte[0]);
            Object warm = SeqFill.INSTANCE.catchUp(
                serverOpener(), serverUrl, f, 0, 49999, SeqFill.MAX_CATCH_UP_BYTES);
            check(warm instanceof SeqFill.Outcome.Advanced, "warm-up advances");
            check(java.util.Arrays.equals(
                java.nio.file.Files.readAllBytes(f.toPath()),
                slice(serverContent, 0, (int) f.length())), "warm-up bytes exact");
            RangeServe.Decision d = RangeServe.INSTANCE.decide(
                RangeServe.INSTANCE.parseRange("bytes=300000-349999"), f.length(), 400 * 1024, true);
            check(d instanceof RangeServe.Decision.FetchSpan, "gap is fetch");
            RangeServe.Decision.FetchSpan fs = (RangeServe.Decision.FetchSpan) d;
            check(!fs.getStore(), "gap never stores directly");
            Object o = SeqFill.INSTANCE.catchUp(
                serverOpener(), serverUrl, f, f.length(), 349999, SeqFill.MAX_CATCH_UP_BYTES);
            check(o instanceof SeqFill.Outcome.Advanced, "catch-up advances");
            byte[] file = java.nio.file.Files.readAllBytes(f.toPath());
            check(file.length > 349999, "prefix covers span end");
            check(java.util.Arrays.equals(slice(file, 300000, 350000),
                slice(serverContent, 300000, 350000)), "span bytes exact");
            // Re-decide now serves from file (what AudioCache does next).
            d = RangeServe.INSTANCE.decide(
                RangeServe.INSTANCE.parseRange("bytes=300000-349999"), file.length, 400 * 1024, true);
            check(d instanceof RangeServe.Decision.ServeSpan, "span serves locally after catch-up");

            // 2. Cap respected: huge target, tiny cap → bounded partial
            // progress (Advanced below target — the serve check still
            // 502s since the span isn't covered).
            serverCursor.set(0);
            java.io.File f2 = tempFile(new byte[0]);
            Object warm2 = SeqFill.INSTANCE.catchUp(
                serverOpener(), serverUrl, f2, 0, 9999, SeqFill.MAX_CATCH_UP_BYTES);
            check(warm2 instanceof SeqFill.Outcome.Advanced, "cap-test warm-up advances");
            Object o2 = SeqFill.INSTANCE.catchUp(
                serverOpener(), serverUrl, f2, f2.length(), 350000, 50000);
            check(o2 instanceof SeqFill.Outcome.Advanced, "cap-hit still reports progress");
            long grown = f2.length();
            check(grown < 350000, "cap stops runaway (target unreached)");
            check(grown <= 10000 + 50000 + 65536, "growth bounded by cap");

            // 3. Fresh full catch-up from zero works (normalized bytes=0- path).
            serverCursor.set(0);
            java.io.File f3 = tempFile(new byte[0]);
            Object o3 = SeqFill.INSTANCE.catchUp(
                serverOpener(), serverUrl, f3, 0, 200000, SeqFill.MAX_CATCH_UP_BYTES);
            check(o3 instanceof SeqFill.Outcome.Advanced, "full catch-up advances");
            check(java.util.Arrays.equals(
                java.nio.file.Files.readAllBytes(f3.toPath()),
                slice(serverContent, 0, (int) f3.length())), "full prefix exact");

            // 4. Dead opener → Unreachable, file untouched.
            java.io.File f4 = tempFile(slice(serverContent, 0, 10000));
            Object o4 = SeqFill.INSTANCE.catchUp(
                (url, range) -> null, serverUrl, f4, 10000, 200000, SeqFill.MAX_CATCH_UP_BYTES);
            check(o4 instanceof SeqFill.Outcome.Unreachable, "dead opener unreachable");
            eq(f4.length(), 10000L, "dead opener writes nothing");

            // 5. Stale txp restart: prefix exists locally but the server
            // cursor sits at 0 (fresh URL after re-resolve) — first window
            // 403s, catch-up deletes and re-walks from zero exactly once.
            serverCursor.set(0);
            java.io.File f5 = tempFile(slice(serverContent, 0, 50000));
            Object o5 = SeqFill.INSTANCE.catchUp(
                serverOpener(), serverUrl, f5, 50000, 349999, SeqFill.MAX_CATCH_UP_BYTES);
            check(o5 instanceof SeqFill.Outcome.Advanced, "restart recovers");
            check(java.util.Arrays.equals(
                java.nio.file.Files.readAllBytes(f5.toPath()),
                slice(serverContent, 0, (int) f5.length())), "restarted bytes exact");

            // 6. Refused full fetch recovers the whole file (the first-play
            // path on throttled links: full GET 403s, windows fill it).
            serverCursor.set(0);
            java.io.File f6 = tempFile(new byte[0]);
            Object o6 = SeqFill.INSTANCE.catchUp(
                serverOpener(), serverUrl, f6, 0, Long.MAX_VALUE, SeqFill.MAX_CATCH_UP_BYTES);
            check(o6 instanceof SeqFill.Outcome.Advanced, "full recovery advances");
            check(java.util.Arrays.equals(
                java.nio.file.Files.readAllBytes(f6.toPath()), serverContent),
                "recovered file byte-exact");
        } finally {
            stopSeqServer();
        }
    }

    // -- Catch-up span parsing ----------------------------------------------------------------------
    static void catchUpSpanTests() {
        // Refused full fetch → whole file from zero.
        kotlin.Pair<Long, Long> p = SeqFill.INSTANCE.catchUpSpan(null, -1);
        eq(p.getFirst(), 0L, "full span start");
        eq(p.getSecond(), Long.MAX_VALUE, "full span runs to EOF");
        // Bounded span → its end.
        p = SeqFill.INSTANCE.catchUpSpan("bytes=300000-349999", 400000);
        eq(p.getFirst(), 300000L, "bounded start");
        eq(p.getSecond(), 349999L, "bounded end");
        // Open span → known total, else EOF.
        p = SeqFill.INSTANCE.catchUpSpan("bytes=300000-", 400000);
        eq(p.getFirst(), 300000L, "open start");
        eq(p.getSecond(), 399999L, "open end is total");
        p = SeqFill.INSTANCE.catchUpSpan("bytes=300000-", -1);
        eq(p.getSecond(), Long.MAX_VALUE, "open unknown runs to EOF");
        // Suffix → absolute tail, else EOF.
        p = SeqFill.INSTANCE.catchUpSpan("bytes=-500", 400000);
        eq(p.getFirst(), 399500L, "suffix start");
        eq(p.getSecond(), 399999L, "suffix end");
        p = SeqFill.INSTANCE.catchUpSpan("bytes=-500", -1);
        eq(p.getFirst(), 0L, "suffix unknown starts at zero");
        // Garbage → null (caller 502s).
        check(SeqFill.INSTANCE.catchUpSpan("bytes=abc-", -1) == null, "garbage span nulls");
    }
}
