package com.spotifydx.app

/**
 * Pure serving logic for the local Range-proxy ([AudioCache]).
 *
 * Dependency-free (no Android imports) so the decisions are unit-runnable
 * on any JVM — the proxy's IO shell ([AudioCache]) only executes what
 * these functions decide. Every rule here exists because the previous
 * inline logic broke real playback:
 *
 * - Seeks beyond the cached prefix DELETED the valid partial and teed
 *   tail bytes at file offset 0 (corrupt cache + corrupt stream). Opus
 *   in WebM needs tail spans (Cues live at the end) while MP4/AAC
 *   carries its header up front — so opus died and aac played.
 * - 206 responses claimed `Content-Length` for spans the file didn't
 *   have yet (truncated body, player stalls waiting for the rest).
 * - 206 `Content-Range` reported `bytes 0-…` while serving tail bytes.
 * - Full-file serves answered 206 instead of 200; 416s lacked the
 *   mandatory `Content-Range`; suffix ranges (`bytes=-N`) were misparsed
 *   as full requests; everything was `application/octet-stream`.
 */
object RangeServe {
    /** Parsed Range spec: exactly one of start / suffix is set. */
    data class RangeReq(
        val start: Long?,
        val end: Long?,
        val suffix: Long?,
    ) {
        val isFull: Boolean get() = start == null && suffix == null
    }

    /**
     * Parses a Range header value (`bytes=A-B`, `bytes=A-`, `bytes=-N`).
     * Garbage (or null) → full request — never throw on player input.
     */
    fun parseRange(spec: String?): RangeReq {
        val s = spec?.trim()?.removePrefix("bytes=")?.trim() ?: return RangeReq(null, null, null)
        if (s.startsWith("-")) {
            val n = s.drop(1).toLongOrNull()
            return if (n != null && n > 0) RangeReq(null, null, n) else RangeReq(null, null, null)
        }
        val (a, b) = (s.split("-", limit = 2) + listOf("", "")).take(2)
        val start = a.toLongOrNull()
        if (start == null || start < 0) return RangeReq(null, null, null)
        return RangeReq(start, b.toLongOrNull(), null)
    }

    /** Serving decision. `have` = cached prefix bytes; `total` = -1 unknown. */
    sealed interface Decision {
        /** Serve [start]..[end] straight from the file (total -1 unknown). */
        data class ServeSpan(val start: Long, val end: Long, val total: Long) : Decision

        /**
         * Fetch upstream with [rangeHeader] (`bytes=A-B`, `bytes=A-`,
         * `bytes=-N`, or null for a full fetch). When [store] is true the
         * span is teed to disk ([deleteFirst] drops a stale partial);
         * spans are only ever stored at offset 0 (restart) or exactly at
         * `have` (append) — a seek-ahead gap is NEVER written into the
         * linear prefix file and NEVER deletes it.
         */
        data class FetchSpan(
            val rangeHeader: String?,
            val store: Boolean,
            val deleteFirst: Boolean,
        ) : Decision

        /** Fail with [code] (404/416/502); total -1 renders `*` in the range. */
        data class Fail(val code: Int, val total: Long) : Decision
    }

    /**
     * Decides how to serve [req] given [have] cached prefix bytes,
     * [total] known length (-1 unknown), and upstream availability.
     */
    fun decide(req: RangeReq, have: Long, total: Long, hasUpstream: Boolean): Decision {
        // Suffix range: needs the total to resolve; otherwise forward
        // verbatim upstream (or 416 with unknown total and no upstream).
        req.suffix?.let { n ->
            if (total > 0) return decideRange(total - n, total - 1, have, total, hasUpstream)
            if (hasUpstream) return Decision.FetchSpan("bytes=-$n", store = false, deleteFirst = false)
            return Decision.Fail(416, -1)
        }
        val start = req.start
        if (start == null) {
            // Full fetch: complete file serves whole; a partial prefix
            // restarts the fill live (delete + tee from zero) so the
            // player gets start→real-EOS instead of a closed stub — a
            // stub answers EOS at the cached end and the player skips
            // there (fixed-timestamp skips). Without upstream the stub
            // is all there is (better partial playback than a 502).
            if (total > 0 && have >= total && have > 0) {
                return Decision.ServeSpan(0, total - 1, total)
            }
            if (have > 0) {
                return if (hasUpstream) {
                    Decision.FetchSpan(null, store = true, deleteFirst = true)
                } else if (total > 0) {
                    Decision.ServeSpan(0, have - 1, total)
                } else {
                    Decision.ServeSpan(0, have - 1, -1)
                }
            }
            return if (hasUpstream) {
                Decision.FetchSpan(null, store = true, deleteFirst = false)
            } else {
                Decision.Fail(502, -1)
            }
        }
        return decideRange(start, req.end, have, total, hasUpstream)
    }

    private fun decideRange(
        rawStart: Long,
        rawEnd: Long?,
        have: Long,
        total: Long,
        hasUpstream: Boolean,
    ): Decision {
        val start = rawStart.coerceAtLeast(0)
        if (total > 0) {
            val end = minOf(rawEnd ?: (total - 1), total - 1)
            if (start >= total || (rawEnd != null && end < start)) {
                return Decision.Fail(416, total)
            }
            // Entire span cached → file.
            if (have > 0 && end < have) {
                return Decision.ServeSpan(start, end, total)
            }
            if (!hasUpstream) {
                // No upstream: serve what exists (clamped), else 416.
                if (have <= 0 || start >= have) return Decision.Fail(416, total)
                return Decision.ServeSpan(start, have - 1, total)
            }
            // Exact append continues the prefix on disk…
            if (start == have) {
                val h = if (rawEnd != null) "bytes=$start-$end" else "bytes=$start-"
                return Decision.FetchSpan(h, store = true, deleteFirst = false)
            }
            // …restart-from-zero replaces a stale partial…
            if (start == 0L) {
                val h = if (rawEnd != null) "bytes=0-$end" else null
                return Decision.FetchSpan(h, store = true, deleteFirst = have > 0)
            }
            // …anything else is a gap: fetch the span, touch nothing.
            val h = if (rawEnd != null) "bytes=$start-$end" else "bytes=$start-"
            return Decision.FetchSpan(h, store = false, deleteFirst = false)
        }
        // Total unknown: serve cached spans, else fetch without storing
        // (nothing verifiable to tee into the prefix file).
        if (have > 0 && start < have) {
            val end = if (rawEnd != null) minOf(rawEnd, have - 1) else have - 1
            if (end >= start) return Decision.ServeSpan(start, end, -1)
        }
        if (!hasUpstream) {
            return if (have > 0) {
                Decision.ServeSpan(minOf(start, have - 1), have - 1, -1)
            } else {
                Decision.Fail(502, -1)
            }
        }
        if (start == 0L || start == have) {
            val h = if (rawEnd != null) "bytes=$start-$rawEnd" else if (start == 0L) null else "bytes=$start-"
            return Decision.FetchSpan(h, store = true, deleteFirst = start == 0L && have > 0)
        }
        val h = if (rawEnd != null) "bytes=$start-$rawEnd" else "bytes=$start-"
        return Decision.FetchSpan(h, store = false, deleteFirst = false)
    }

    /** Response head: status line + headers (never throws). */
    data class Head(val code: Int, val message: String, val headers: Map<String, String>)

    private fun baseHeaders(mime: String): MutableMap<String, String> = mutableMapOf(
        "Content-Type" to mime,
        "Accept-Ranges" to "bytes",
        "Connection" to "close",
    )

    /**
     * Head for a file span. Full requests for a complete file answer 200
     * (no Content-Range); everything else answers 206 with a truthful
     * Content-Range and an exact Content-Length — never more than the
     * bytes actually following.
     */
    fun fileHead(start: Long, end: Long, total: Long, mime: String, fullRequest: Boolean): Head {
        val h = baseHeaders(mime)
        if (fullRequest && total > 0 && start == 0L && end == total - 1) {
            h["Content-Length"] = "$total"
            return Head(200, "OK", h)
        }
        if (total > 0) {
            h["Content-Range"] = "bytes $start-$end/$total"
            h["Content-Length"] = "${end - start + 1}"
            return Head(206, "Partial Content", h)
        }
        return Head(200, "OK", h)
    }

    /**
     * Head for an upstream fetch: relays the upstream span truthfully.
     * [code] is the upstream status, [contentRange]/[contentLength] its
     * headers, [reqStart] the span start we asked for. Unknown spans stay
     * close-delimited (no invented lengths, no offset lies).
     */
    fun fetchHead(
        reqStart: Long?,
        code: Int,
        contentRange: String?,
        contentLength: String?,
        mime: String,
    ): Head {
        val h = baseHeaders(mime)
        if (code == 206) {
            // Relay the upstream Content-Range verbatim when it parses —
            // it describes the bytes actually coming.
            val span = parseContentRange(contentRange)
            if (span != null) {
                val (s, e, t) = span
                if (reqStart == null || reqStart == s) {
                    h["Content-Range"] = "bytes $s-$e/$t"
                    h["Content-Length"] = "${e - s + 1}"
                    return Head(206, "Partial Content", h)
                }
            }
            return Head(206, "Partial Content", h)
        }
        contentLength?.toLongOrNull()?.takeIf { it >= 0 }?.let { h["Content-Length"] = "$it" }
        return Head(200, "OK", h)
    }

    /** Parses `bytes S-E/T` (T may be `*` → -1). Null on garbage. */
    fun parseContentRange(cr: String?): Triple<Long, Long, Long>? {
        if (cr == null) return null
        val m = Regex("""bytes\s+(\d+)-(\d+)/(\d+|\*)""").find(cr.trim()) ?: return null
        val (s, e, t) = m.destructured
        return Triple(s.toLong(), e.toLong(), if (t == "*") -1 else t.toLong())
    }

    /**
     * Head for a live full fill (player asked whole file, upstream is
     * being teed through right now): always 200 close-delimited, never a
     * length. A declared length the stalled transfer can't satisfy turns
     * into a phantom EOS — the player skips there. EOS must mean the real
     * end (or a genuine failure, which surfaces as an error, not a skip).
     */
    fun liveFullHead(mime: String): Head {
        val h = baseHeaders(mime)
        return Head(200, "OK", h)
    }

    /** 416 head (RFC 9110: Content-Range mandatory; `*` when total unknown). */
    fun unsatisfiableHead(total: Long): Head {
        val h = baseHeaders("application/octet-stream")
        h["Content-Range"] = if (total > 0) "bytes */$total" else "bytes */*"
        return Head(416, "Range Not Satisfiable", h)
    }

    /**
     * Player-facing MIME from a resolver format tag (`provider/format/…`
     * quality keys carry e.g. `opus`, `mp4`, `m4a`, `mp3`, `flac`, `ogg`).
     * Unknown → octet-stream (sniffing fallback, today's behavior).
     */
    fun mimeForFormat(format: String?): String = when (format?.lowercase()) {
        "opus" -> "audio/webm"
        "mp4", "m4a", "aac" -> "audio/mp4"
        "mp3" -> "audio/mpeg"
        "flac" -> "audio/flac"
        "ogg", "oga" -> "audio/ogg"
        else -> "application/octet-stream"
    }

    /**
     * Resolves a redirect `Location` against the request URL (RFC 3986 via
     * `java.net.URI` — `java.net.URL` mis-resolves query-only refs).
     * Returns null for non-HTTP targets (never follow rtsp/file/custom
     * schemes out of the proxy). Pure — the hop cap lives with the caller.
     */
    fun resolveRedirect(currentUrl: String, location: String): String? {
        return try {
            // Query-only refs keep the base path (the JDK resolvers strip
            // the last segment here, against RFC 3986 §5.2.2).
            if (location.startsWith("?")) {
                val base = java.net.URI(currentUrl)
                if (base.scheme != "http" && base.scheme != "https") return null
                return java.net.URI(
                    base.scheme, base.authority,
                    base.path.ifEmpty { "/" }, location.drop(1), null,
                ).toString()
            }
            val next = java.net.URI(currentUrl).resolve(location)
            if (next.scheme != "http" && next.scheme != "https") return null
            next.toString()
        } catch (e: Exception) {
            null
        }
    }
}

/**
 * Single-flight tee registration for the prefix cache (plain JVM, no
 * Android imports — unit-testable, incl. threaded).
 *
 * The serving lock must never span network transfers: a second player
 * connection (e.g. a WebM tail seek for Cues, required to COMPLETE
 * prepare) must not queue behind a minutes-long full-track fill, or
 * prepare starves into the watchdog. So file mutation is coordinated
 * here — one tee session per key, claimed atomically in microseconds —
 * while all network IO runs lock-free. ServeSpan sends need no
 * coordination at all (the prefix file only grows; sends clamp to the
 * live length).
 */
object TeeGate {
    private val active = java.util.concurrent.ConcurrentHashMap<String, Boolean>()

    /** Claims the tee slot for [key]; false when another fill owns it. */
    fun tryAcquire(key: String): Boolean = active.putIfAbsent(key, true) == null

    fun release(key: String) {
        active.remove(key)
    }
}

/**
 * Sequential catch-up fill (plain JVM: `java.net` + `java.io` only —
 * HttpServer-testable). Some upstreams (YouTube throttled `gir=yes`
 * URLs) enforce strictly sequential windowed access: any non-sequential
 * span 403s, full GETs without Range 403 too. Blindly forwarding such
 * spans dead-ends prepares; instead consume forward from the valid
 * prefix end (teeing — every byte stays a valid prefix) until the wanted
 * span is cached, then serve locally.
 */
object SeqFill {
    /** Hard cap per catch-up (a runaway span must fail bounded, not fill disks). */
    const val MAX_CATCH_UP_BYTES = 64L * 1024 * 1024

    /**
     * Window per upstream request. Throttled links (`gir=yes`) 403 open
     * ranges and oversized windows (measured: 256–512KB pass, 1MB and
     * open fail) while serving sequential bounded windows instantly —
     * so catch-up always walks in windows, never open-ended.
     */
    const val WINDOW_BYTES = 256L * 1024

    /** Opens a connection (tests inject fakes; production passes the redirect loop). */
    fun interface Opener {
        fun open(url: String, rangeHeader: String?): java.net.HttpURLConnection?
    }

    sealed interface Outcome {
        /** Prefix grew to [newHave] ([total] best-known, -1 unknown). */
        data class Advanced(val newHave: Long, val total: Long) : Outcome

        /** No progress possible (no route, repeated 403, cap hit, IO dead). */
        data object Unreachable : Outcome
    }

    /**
     * Advances the prefix file at [file] (currently [have] valid bytes)
     * forward until [targetEnd] (inclusive) is cached, EOF, or [capBytes]
     * is consumed. Only appends at the live end (re-snapped every lap, so
     * a concurrent completer can't tear it); never deletes, never gaps.
     * Laps end on EOF/stall/cap; success is measured by progress, not by
     * reaching the target (open-ended spans end at EOF by definition).
     */
    @JvmOverloads
    fun catchUp(
        open: Opener,
        url: String,
        file: java.io.File,
        have: Long,
        targetEnd: Long,
        capBytes: Long = MAX_CATCH_UP_BYTES,
    ): Outcome {
        fun len(): Long = if (file.isFile) file.length() else 0
        if (len() > targetEnd) return Outcome.Advanced(len(), -1)
        var total: Long = -1
        var consumed = 0L
        // Entry floor: the file must never shrink beneath it (a replaced
        // file aborts the run). Reset to 0 on a cursor restart below.
        var floor = have
        // At most one cursor restart: a 403 means the server's transfer
        // cursor isn't where the prefix ends (fresh/stale txp) — delete
        // and re-walk from 0 (replays are allowed), exactly once.
        var restarts = 0
        while (consumed < capBytes) {
            val cursor = len()
            if (cursor < floor) return progressOrDead(cursor, floor, total)
            if (cursor > targetEnd) return Outcome.Advanced(cursor, total)
            // Bounded window per request (open/oversized spans 403 on
            // throttled links); clamped to the target span.
            val windowEnd = minOf(cursor + WINDOW_BYTES - 1, targetEnd)
            val consumedBefore = consumed
            val conn = open.open(url, "bytes=$cursor-$windowEnd")
                ?: return progressOrDead(cursor, floor, total)
            try {
                val code = try {
                    conn.responseCode
                } catch (e: Exception) {
                    return progressOrDead(cursor, floor, total)
                }
                if (code == 403 && restarts == 0) {
                    restarts++
                    floor = 0
                    file.delete()
                    continue
                }
                if (code != 200 && code != 206) {
                    return progressOrDead(cursor, floor, total)
                }
                // A 200 here restarts from zero — only usable at offset 0.
                if (code == 200 && cursor > 0) return progressOrDead(cursor, floor, total)
                conn.getHeaderField("Content-Range")?.substringAfter("/")?.trim()
                    ?.toLongOrNull()?.let { if (it > 0) total = it }
                if (total <= 0) {
                    conn.getHeaderField("Content-Length")?.toLongOrNull()?.let {
                        total = it + cursor
                    }
                }
                val raf = try {
                    java.io.RandomAccessFile(file, "rw").apply { seek(length()) }
                } catch (e: Exception) {
                    return progressOrDead(cursor, floor, total)
                }
                try {
                    val buf = ByteArray(64 * 1024)
                    conn.inputStream.use { ins ->
                        while (true) {
                            if (len() > targetEnd) break
                            if (consumed >= capBytes) break
                            val n = try {
                                ins.read(buf)
                            } catch (e: Exception) {
                                break
                            }
                            if (n < 0) break
                            try {
                                raf.write(buf, 0, n)
                            } catch (e: Exception) {
                                break
                            }
                            consumed += n
                        }
                    }
                } finally {
                    runCatching { raf.close() }
                }
                // A lap with zero progress (empty 206 at EOF, repeated
                // refusals) must terminate, not spin: success is measured
                // by growth, and the caller serves iff the span is covered.
                if (consumed == consumedBefore && len() == cursor) {
                    return progressOrDead(cursor, floor, total)
                }
            } finally {
                runCatching { conn.disconnect() }
            }
        }
        return progressOrDead(len(), floor, total)
    }

    private fun progressOrDead(len: Long, have: Long, total: Long): Outcome =
        if (len > have) Outcome.Advanced(len, total) else Outcome.Unreachable

    /**
     * Catch-up span for a refused fetch: null header (refused full fetch)
     * recovers from 0 to EOF; bounded spans to their end; open spans to
     * the known total (or EOF); suffixes resolve against the known total
     * (unknown total runs to EOF). Returns (spanStart, targetEnd), or null
     * when unparseable. Pure — AudioCache serves locally afterwards iff
     * the prefix covers spanStart.
     */
    fun catchUpSpan(rangeHeader: String?, knownTotal: Long): Pair<Long, Long>? {
        if (rangeHeader == null) return 0L to Long.MAX_VALUE
        val spec = rangeHeader.removePrefix("bytes=")
        if (spec.startsWith("-")) {
            if (knownTotal <= 0) return 0L to Long.MAX_VALUE
            val n = spec.drop(1).toLongOrNull() ?: return null
            return (knownTotal - n).coerceAtLeast(0) to (knownTotal - 1)
        }
        val start = spec.substringBefore("-").toLongOrNull() ?: return null
        val endRaw = spec.substringAfter("-", "").takeIf { it.isNotEmpty() }?.toLongOrNull()
        val end = endRaw ?: if (knownTotal > 0) knownTotal - 1 else Long.MAX_VALUE
        return start to end
    }
}
