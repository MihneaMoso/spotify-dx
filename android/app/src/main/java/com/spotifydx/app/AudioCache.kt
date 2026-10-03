package com.spotifydx.app

import android.content.Context
import android.util.Log
import java.io.File
import java.io.RandomAccessFile
import java.net.HttpURLConnection
import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket
import java.net.URL
import java.util.Collections
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Executors
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext

/**
 * Production audio cache (Echo Music's two-tier player cache, adapted —
 * MediaPlayer has no ExoPlayer-style `CacheDataSource` hook, so the
 * equivalent is a local Range-proxy + LRU file store):
 *
 * - The player streams via `http://127.0.0.1:<port>/<key>`; the proxy
 *   forwards upstream with `Range`, teeing bytes to disk while serving.
 *   Replays and seek-backs reuse cached spans; partial plays resume from
 *   their byte offset (fresh resolve only for the missing tail — stream
 *   URLs expire, so cache keys are track-id based with quality sidecars).
 * - Complete files play straight from disk: instant start, fully offline
 *   (no resolve at all).
 * - 1 GB cap, LRU by last-played; tracks played ≥ [PIN_PLAYS] times are
 *   pinned (exempt unless everything is pinned). Stats live in the
 *   `cache_entries` Room table; files in `filesDir/audiocache`.
 *
 * Threading: one daemon acceptor + pooled connection handlers.
 * The per-key lock NEVER spans network IO — it covers only the atomic
 * decide-and-claim instant (snapshot + tee registration, milliseconds).
 * File mutation is coordinated by TeeGate (one tee session per key);
 * body transfer runs lock-free, so a second player connection (e.g. a
 * WebM tail seek for Cues, required to complete prepare) never queues
 * behind a minutes-long fill. File sends clamp to the live length, so
 * races resolve as truthful short spans, never corrupt ones.
 */
object AudioCache {
    private const val TAG = "SpotifyDxCache"

    /** Storage cap (confirmed hybrid: 1 GB LRU, cache-all). */
    const val MAX_BYTES = 1024L * 1024 * 1024

    /** Play-count pinning threshold (confirmed hybrid). */
    const val PIN_PLAYS = 3

    private const val DIR = "audiocache"
    private const val CHUNK = 64 * 1024
    private const val UA = "stagefright/1.2 (Linux;Android 14)"
    private const val MAX_REDIRECTS = 5
    private val REDIRECT_CODES = setOf(301, 302, 303, 307, 308)

    sealed interface Source {
        data class Disk(val path: String) : Source
        data class Proxy(val url: String) : Source
    }

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val pool = Executors.newCachedThreadPool { r ->
        Thread(r, "audio-cache").apply { isDaemon = true }
    }

    @Volatile
    private var appCtx: Context? = null

    /** Process-lifetime init (call from Application.onCreate). */
    fun init(ctx: Context) {
        if (appCtx == null) appCtx = ctx.applicationContext
    }

    private fun requireCtx(): Context =
        requireNotNull(appCtx) { "AudioCache.init(applicationContext) missing" }

    @Volatile
    private var port: Int = 0

    /** Host only (never query — URLs carry session tokens). */
    private fun hostOf(url: String): String = runCatching {
        URL(url).host
    }.getOrDefault("bad-url")

    /** Fresh upstream URL per key (registered on every play start). */
    private val upstream = ConcurrentHashMap<String, String>()

    /** Resolver format tag per key (`provider/format/…` quality keys) for MIME. */
    private val formats = ConcurrentHashMap<String, String>()

    private val locks = ConcurrentHashMap<String, Any>()

    /** Keys with an active serving stream (exempt from eviction). */
    private val serving =
        Collections.newSetFromMap(ConcurrentHashMap<String, Boolean>())

    private fun lockFor(key: String): Any = locks.getOrPut(key) { Any() }

    private fun dir(): File = requireCtx().filesDir.resolve(DIR).apply { mkdirs() }

    private fun fileFor(key: String): File = dir().resolve("$key.audio")

    private fun keyFor(track: Track): String {
        val clean = track.id.replace(Regex("[^A-Za-z0-9_-]"), "_").take(80)
        return clean.ifEmpty { "noid" }
    }

    private fun dao() = AppDb.get(requireCtx()).audioCache()

    /**
     * Playback source for a freshly resolved track. Complete files hit
     * disk directly; anything else streams through the proxy (which fills
     * the file as it serves). Never throws — falls back to proxy.
     *
     * Suspend (Room is main-safe): playUrl calls this from the Main scope,
     * so no runBlocking here. The one-time ServerSocket bind in
     * ensureServer stays inline (sub-ms, first play only).
     */
    suspend fun playbackSource(
        ctx: Context,
        track: Track,
        qualityKey: String,
        upstreamUrl: String,
    ): Source {
        ensureServer(ctx)
        val key = keyFor(track)
        if (track.id.isEmpty()) {
            // Unkeyable stream: pass-through proxy under an ephemeral key,
            // nothing cached.
            val liveKey = "$key-live"
            upstream[liveKey] = upstreamUrl
            rememberFormat(liveKey, qualityKey)
            return Source.Proxy("http://127.0.0.1:$port/$liveKey")
        }
        val entry = try {
            dao().entry(key)
        } catch (e: Exception) {
            Log.w(TAG, "index read failed, proxying: ${e.message}")
            null
        }
        val file = fileFor(key)
        if (entry?.complete == true && entry.qualityKey == qualityKey &&
            file.isFile && file.length() == entry.bytes && entry.bytes > 0
        ) {
            touchAsync(key)
            Log.i(TAG, "HIT $key (${entry.bytes / 1024} KB, plays=${entry.playCount})")
            return Source.Disk(file.absolutePath)
        }
        if ((entry != null && entry.qualityKey != qualityKey) ||
            (entry?.complete == true && (!file.isFile || file.length() != entry.bytes))
        ) {
            // Stale tier or orphaned row: drop and start over.
            scope.launch {
                runCatching { file.delete() }
                runCatching { dao().delete(key) }
            }
        }
        upstream[key] = upstreamUrl
        notePlayAsync(key, qualityKey)
        rememberFormat(key, qualityKey)
        Log.i(TAG, "MISS $key (proxying, have=${file.takeIf { it.isFile }?.length() ?: 0} B)")
        return Source.Proxy("http://127.0.0.1:$port/$key")
    }

    /** Remembers the resolver format tag for player-facing MIME types. */
    private fun rememberFormat(key: String, qualityKey: String) {
        val format = qualityKey.split("/").getOrNull(1)?.takeIf { it.isNotEmpty() }
        if (format == null) formats.remove(key) else formats[key] = format
    }

    private fun mimeFor(key: String): String =
        RangeServe.mimeForFormat(formats[key])

    /** Fire-and-forget play stats (count + recency drive LRU + pinning). */
    private fun notePlayAsync(key: String, qualityKey: String) {
        scope.launch {
            runCatching {
                val dao = dao()
                val prev = dao.entry(key)
                val fileLen = fileFor(key).takeIf { it.isFile }?.length() ?: 0
                dao.upsert(
                    (prev ?: CacheEntry(trackId = key)).copy(
                        qualityKey = qualityKey,
                        bytes = if (prev != null) prev.bytes else fileLen,
                        playCount = (prev?.playCount ?: 0) + 1,
                        lastPlayedMs = System.currentTimeMillis(),
                    ),
                )
            }
        }
    }

    private fun touchAsync(key: String) {
        scope.launch {
            runCatching {
                val dao = dao()
                dao.entry(key)?.let { dao.upsert(it.copy(lastPlayedMs = System.currentTimeMillis())) }
            }
        }
    }

    /** Cache size + pin count for the Settings screen. */
    suspend fun sizeInfo(): Pair<Long, Int> = withContext(Dispatchers.IO) {
        val all = runCatching { dao().allOrdered() }.getOrElse { emptyList() }
        Pair(all.sumOf { it.bytes }, all.count { it.playCount >= PIN_PLAYS })
    }

    /** Clears files + index (active streams fail over to replay). */
    suspend fun clear() {
        withContext(Dispatchers.IO) {
            runCatching { dir().listFiles()?.forEach { it.delete() } }
            runCatching { dao().clear() }
        }
        Log.i(TAG, "cache cleared")
    }

    // -- Local Range server -----------------------------------------------------

    private fun ensureServer(ctx: Context) {
        appCtx = ctx.applicationContext
        if (port != 0) return
        synchronized(this) {
            if (port != 0) return
            val ss = ServerSocket(0, 50, InetAddress.getByName("127.0.0.1"))
            port = ss.localPort
            pool.execute {
                while (!ss.isClosed) {
                    try {
                        val s = ss.accept()
                        pool.execute { handle(s) }
                    } catch (e: Exception) {
                        if (!ss.isClosed) Log.w(TAG, "accept: ${e.message}")
                    }
                }
            }
            Log.i(TAG, "proxy on 127.0.0.1:$port")
        }
    }

    private fun handle(sock: Socket) {
        sock.use { s ->
            s.soTimeout = 15000
            val input = s.getInputStream().bufferedReader()
            val requestLine = input.readLine() ?: return
            val parts = requestLine.split(" ")
            if (parts.size < 2 || parts[0] != "GET") {
                reply(s, 405, "Method Not Allowed", emptyMap(), null)
                return
            }
            val key = parts[1].trimStart('/').substringBefore('?').take(128)
                // Re-sanitize like keyFor(): the port is reachable from
                // loopback, so path traversal ("../") must die here even
                // though only our player should ever connect. Replace-only
                // (no trim): legit keys pass through byte-identical.
                .replace(Regex("[^A-Za-z0-9_.-]"), "_")
            var rangeSpec: String? = null
            while (true) {
                val line = input.readLine() ?: break
                if (line.isEmpty()) break
                if (line.startsWith("Range:", ignoreCase = true)) {
                    rangeSpec = line.substringAfter(":").trim()
                }
            }
            if (key.isEmpty()) {
                reply(s, 404, "Not Found", emptyMap(), null)
                return
            }
            // Pure parse (suffix ranges, garbage → full request, never throws).
            val req = RangeServe.parseRange(rangeSpec)
            try {
                serve(s, key, req)
            } finally {
                // Ephemeral pass-through keys never repeat: drop them.
                if (key.endsWith("-live")) {
                    upstream.remove(key)
                    formats.remove(key)
                }
            }
        }
    }

    private fun serve(sock: Socket, key: String, req: RangeServe.RangeReq) {
        // Lock-free fast path first (read-only snapshot); the locked path
        // below never spans network IO, so a second player connection
        // (WebM tail seek for Cues, needed to COMPLETE prepare) never
        // queues behind a minutes-long fill. Runs on pool threads:
        // blocking index reads are fine.
        serving.add(key)
        try {
            try {
                if (tryServeCached(sock, key, req)) return
            } catch (e: Exception) {
                Log.w(TAG, "fast path $key: ${e.message}")
            }
            serveLocked(sock, key, req)
        } catch (e: Exception) {
            Log.w(TAG, "serve $key: ${e.message}")
        } finally {
            serving.remove(key)
        }
        scope.launch { evictIfNeeded() }
    }

    /** Serves a fully-cached span with no locking (read-only snapshot). */
    private fun tryServeCached(sock: Socket, key: String, req: RangeServe.RangeReq): Boolean {
        val file = fileFor(key)
        if (!file.isFile) return false
        val have = file.length()
        if (have == 0L) return false
        val total = runBlocking(Dispatchers.IO) { dao().entry(key) }?.totalBytes ?: -1
        // The file only ever holds a valid [0, have) prefix (gaps are
        // never teed, partials never deleted mid-stream), so a ServeSpan
        // against this snapshot stays valid even as appends land
        // (sendFile clamps to the live length anyway).
        val d = RangeServe.decide(req, have, total, upstream.containsKey(key))
        if (d is RangeServe.Decision.ServeSpan) {
            sendFile(sock, file, d, mimeFor(key), req.isFull)
            return true
        }
        return false
    }

    /**
     * Atomic decide under one short lock hold (no network inside, no slot
     * held across the fetch — the slot is acquired lazily around the
     * actual tee / catch-up instead. Holding it across fetchAndServe
     * deadlocked the in-band catch-up against its own request: 15s in the
     * wait loop, then 502, on every throttled first fetch.)
     */
    private data class Claim(
        val total: Long,
        val decision: RangeServe.Decision,
    )

    private fun serveLocked(sock: Socket, key: String, req: RangeServe.RangeReq) {
        val file = fileFor(key)
        val claim = synchronized(lockFor(key)) {
            val have = if (file.isFile) file.length() else 0
            val total = runBlocking(Dispatchers.IO) { dao().entry(key) }?.totalBytes ?: -1
            val d = RangeServe.decide(req, have, total, upstream.containsKey(key))
            Claim(total, d)
        }
        when (val d = claim.decision) {
            is RangeServe.Decision.ServeSpan ->
                sendFile(sock, file, d, mimeFor(key), req.isFull)
            is RangeServe.Decision.FetchSpan ->
                fetchAndServe(sock, key, file, d, mimeFor(key), claim.total)
            is RangeServe.Decision.Fail ->
                if (d.code == 416) {
                    val h = RangeServe.unsatisfiableHead(d.total)
                    val out = sock.getOutputStream()
                    writeHead(out, h.code, h.message, h.headers)
                    out.flush()
                } else {
                    reply(sock, d.code, "Error", emptyMap(), null)
                }
        }
    }

    /**
     * Opens an upstream connection following redirects (max 5) with
     * [rangeHeader] re-applied per hop. Returns a connected connection or
     * null on transport failure (caller replies 502). The caller owns
     * disconnecting (streamBody does on the body path).
     */
    private fun openUpstream(url: String, rangeHeader: String?): HttpURLConnection? {
        var current = url
        var hops = 0
        while (true) {
            val c = try {
                (URL(current).openConnection() as HttpURLConnection).apply {
                    connectTimeout = 15000
                    readTimeout = 30000
                    setRequestProperty("User-Agent", UA)
                    rangeHeader?.let { setRequestProperty("Range", it) }
                    instanceFollowRedirects = false
                }
            } catch (e: Exception) {
                return null
            }
            val code = try {
                c.connect()
                c.responseCode
            } catch (e: Exception) {
                Log.i(TAG, "open ${hostOf(current)} ${rangeHeader ?: "full"} failed: ${e.message}")
                runCatching { c.disconnect() }
                return null
            }
            if (code !in REDIRECT_CODES) return c
            if (hops >= MAX_REDIRECTS) {
                runCatching { c.disconnect() }
                return null
            }
            val loc = c.getHeaderField("Location")
            c.disconnect()
            if (loc == null) return null
            current = RangeServe.resolveRedirect(current, loc) ?: return null
            hops++
        }
    }

    /**
     * Fetches the decided span upstream and serves it, lock-free (the tee
     * slot was claimed atomically in serveLocked, or this is pass-through).
     * Gaps (seek-ahead) are pure pass-through: the prefix file is NEVER
     * deleted and the span is NEVER teed into it. A 403 on a span fetch
     * (throttled links refuse non-sequential windows) triggers one
     * sequential catch-up + local serve instead of surfacing the 502 that
     * used to starve WebM prepares of their tail Cues.
     */
    private fun fetchAndServe(
        sock: Socket,
        key: String,
        file: File,
        fetch: RangeServe.Decision.FetchSpan,
        mime: String,
        knownTotal: Long,
    ) {
        val url = upstream[key] ?: run {
            reply(sock, 502, "Bad Gateway", emptyMap(), null)
            return
        }
        val store = fetch.store && !key.endsWith("-live")
        // Throttled links 403 full GETs without Range: normalize to
        // bytes=0- (identical semantics). Response handling still treats
        // it as the full fetch the player asked for.
        val fwdHeader = fetch.rangeHeader ?: "bytes=0-"
        val connection = openUpstream(url, fwdHeader) ?: run {
            reply(sock, 502, "Bad Gateway", emptyMap(), null)
            return
        }
        // Already fetched inside openUpstream (cached — no second request).
        val code = try {
            connection.responseCode
        } catch (e: Exception) {
            runCatching { connection.disconnect() }
            reply(sock, 502, "Bad Gateway", emptyMap(), null)
            return
        }
        Log.i(TAG, "fetch $key host=${hostOf(url)} fwd=${fwdHeader} code=$code")
        if (code == 403) {
            // Sequential-only upstream refused the window (throttled
            // links 403 full GETs, open ranges, oversized windows, and
            // non-sequential spans alike): catch the prefix up in bounded
            // sequential windows, then serve locally. One recovery per
            // request — anything unrecoverable stays a 502.
            connection.disconnect()
            Log.i(TAG, "fetch $key 403, catching up")
            if (serveViaCatchUp(sock, key, file, fetch, mime, knownTotal)) return
            reply(sock, 502, "Bad Gateway", emptyMap(), null)
            return
        }
        if (code == 416) {
            connection.disconnect()
            val h = RangeServe.unsatisfiableHead(knownTotal)
            val out = sock.getOutputStream()
            writeHead(out, h.code, h.message, h.headers)
            out.flush()
            return
        }
        if (code != 200 && code != 206) {
            connection.disconnect()
            reply(sock, 502, "Bad Gateway", emptyMap(), null)
            return
        }
        // Span start we asked for (null = full fetch or verbatim suffix):
        // drives the truthfulness check on relayed Content-Range below.
        val askedStart = fetch.rangeHeader
            ?.removePrefix("bytes=")
            ?.substringBefore("-")
            ?.toLongOrNull()
        // Server ignored Range but we asked mid-span: the body starts at 0,
        // not at askedStart. Retee from zero when storing (the stale
        // partial is unusable); otherwise serve without touching the file.
        // The slot is acquired lazily HERE (never held across the fetch —
        // holding it earlier deadlocked the in-band catch-up against its
        // own request); losers transparently become pass-through.
        val rangeIgnored = code == 200 && (askedStart ?: 0) > 0
        val effectiveStart: Long? = if (rangeIgnored) null else askedStart
        var teeHeld = false
        try {
            if (store) {
                teeHeld = TeeGate.tryAcquire(key)
                if (teeHeld && (fetch.deleteFirst || rangeIgnored)) {
                    file.delete()
                }
            }
            streamBody(sock, key, connection, file, code, effectiveStart, teeHeld, mime, fetch.rangeHeader == null)
        } finally {
            if (teeHeld) TeeGate.release(key)
        }
    }

    /**
     * Sequential-catch-up recovery for 403'd spans (throttled links refuse
     * non-sequential windows): tee the valid prefix forward to the span
     * end, then serve locally. Returns false when unrecoverable (caller
     * replies 502). Exactly one recovery per request — no loops.
     */
    private fun serveViaCatchUp(
        sock: Socket,
        key: String,
        file: File,
        fetch: RangeServe.Decision.FetchSpan,
        mime: String,
        knownTotal: Long,
    ): Boolean {
        val url = upstream[key] ?: return false
        // Catch-up span for the refused fetch (pure parse in RangeServe).
        val span = SeqFill.catchUpSpan(fetch.rangeHeader, knownTotal) ?: return false
        val spanStart = span.first
        val spanEnd: Long? = span.second.takeIf { it != Long.MAX_VALUE }
        val targetEnd = span.second
        // Catch-up writes need the tee slot. If a live fill owns it,
        // wait boundedly instead of failing: its own progress may cover
        // the span (re-decide sees it), else the slot frees and we fill.
        // Either way this returns served-or-502 within ~15s, never wedged.
        val deadline = android.os.SystemClock.uptimeMillis() + 15_000
        var advanced: SeqFill.Outcome.Advanced? = null
        var lastOutcome: SeqFill.Outcome = SeqFill.Outcome.Unreachable
        while (true) {
            if (TeeGate.tryAcquire(key)) {
                try {
                    val opener = SeqFill.Opener { u, r -> openUpstream(u, r) }
                    val haveNow = if (file.isFile) file.length() else 0
                    when (val o = SeqFill.catchUp(opener, url, file, haveNow, targetEnd)) {
                        is SeqFill.Outcome.Advanced -> {
                            advanced = o
                            lastOutcome = o
                            upsertProgress(key, file, o.total)
                        }
                        is SeqFill.Outcome.Unreachable -> {
                            lastOutcome = o
                        }
                    }
                } finally {
                    TeeGate.release(key)
                }
                break
            }
            // Slot busy: re-decide on live state (the fill may have covered
            // the span already — then serve without any catch-up at all).
            val haveNow = if (file.isFile) file.length() else 0
            val totalNow =
                runBlocking(Dispatchers.IO) { dao().entry(key) }?.totalBytes ?: knownTotal
            val again = RangeServe.decide(
                RangeServe.RangeReq(spanStart, spanEnd, null), haveNow, totalNow, true,
            )
            if (again is RangeServe.Decision.ServeSpan) {
                sendFile(sock, file, again, mime, false)
                return true
            }
            if (android.os.SystemClock.uptimeMillis() >= deadline) break
            try {
                Thread.sleep(200)
            } catch (e: InterruptedException) {
                break
            }
        }
        val adv = advanced
        val len = if (file.isFile) file.length() else 0
        Log.i(TAG, "catchup $key outcome=$lastOutcome len=$len spanStart=$spanStart")
        if (adv == null || len <= spanStart) return false
        val end = minOf(spanEnd ?: (len - 1), len - 1)
        if (end < spanStart) return false
        val total = adv.total.takeIf { it > 0 } ?: knownTotal
        sendFile(sock, file, RangeServe.Decision.ServeSpan(spanStart, end, total), mime, fetch.rangeHeader == null)
        return true
    }

    /** Persists catch-up progress (bytes + best-known total, rarely complete). */
    private fun upsertProgress(key: String, file: File, total: Long) {
        runBlocking(Dispatchers.IO) {
            runCatching {
                val dao = dao()
                val prev = dao.entry(key)
                val len = file.length()
                val known = if (total > 0) total else (prev?.totalBytes ?: -1)
                dao.upsert(
                    (prev ?: CacheEntry(trackId = key)).copy(
                        bytes = len,
                        totalBytes = known,
                        complete = known > 0 && len >= known,
                    ),
                )
            }
        }
    }

    private fun streamBody(
        sock: Socket,
        key: String,
        conn: HttpURLConnection,
        file: File,
        code: Int,
        spanStart: Long?,
        store: Boolean,
        mime: String,
        fullFetch: Boolean,
    ) {
        // Truthful head from the upstream response (relays Content-Range
        // only when it matches the span asked for — never invents spans).
        // Full live fills always go close-delimited (liveFullHead): any
        // declared length a stalled transfer can't satisfy becomes a
        // phantom EOS with a fixed-timestamp skip. Suffix fetches are NOT
        // full (their span resolves server-side) and keep the relay.
        val head = if (fullFetch) {
            RangeServe.liveFullHead(mime)
        } else {
            RangeServe.fetchHead(
                spanStart,
                code,
                conn.getHeaderField("Content-Range"),
                conn.getHeaderField("Content-Length"),
                mime,
            )
        }
        val out = sock.getOutputStream()
        writeHead(out, head.code, head.message, head.headers)
        Log.i(TAG, "stream $key ${head.code} ${head.headers["Content-Range"] ?: "full"} len=${head.headers["Content-Length"] ?: "?"} ${head.headers["Content-Type"]}")
        // Tee guard: body bytes land at spanStart (0 for full fetches).
        // Anything else means the file and the span disagree — serve only,
        // never write mid-file gaps (that poisoned the prefix cache).
        val base = spanStart ?: 0
        var doStore = store
        var raf: RandomAccessFile? = null
        if (doStore) {
            if (!file.isFile || file.length() != base) {
                if (base == 0L) {
                    file.delete()
                } else {
                    doStore = false
                }
            }
            if (doStore) {
                raf = RandomAccessFile(file, "rw").apply { if (base > 0) seek(base) }
            }
        }
        try {
            val buf = ByteArray(CHUNK)
            conn.inputStream.use { ins ->
                while (true) {
                    val n = ins.read(buf)
                    if (n < 0) break
                    out.write(buf, 0, n)
                    raf?.write(buf, 0, n)
                }
            }
            out.flush()
            Log.i(TAG, "stream $key done stored=$doStore file=${file.length()}")
            if (doStore) {
                val finalLen = file.length()
                val finalTotal =
                    conn.getHeaderField("Content-Range")?.substringAfter("/")?.trim()?.toLongOrNull()
                        ?: conn.getHeaderField("Content-Length")?.toLongOrNull()?.let { it + base }
                        ?: -1
                val complete = finalTotal > 0 && finalLen >= finalTotal
                runBlocking(Dispatchers.IO) {
                    val dao = dao()
                    val prev = dao.entry(key)
                    dao.upsert(
                        (prev ?: CacheEntry(trackId = key)).copy(
                            bytes = finalLen,
                            totalBytes = finalTotal,
                            complete = complete,
                        ),
                    )
                }
                if (complete) Log.i(TAG, "complete $key (${finalLen / 1024} KB)")
            }
        } finally {
            runCatching { raf?.close() }
            runCatching { conn.disconnect() }
        }
    }

    private fun sendFile(
        sock: Socket,
        file: File,
        span: RangeServe.Decision.ServeSpan,
        mime: String,
        fullRequest: Boolean,
    ) {
        // Clamp to the live length: a concurrent restart may have replaced
        // the file after decide. A vanished file aborts silently (the
        // player re-requests); a shortened one serves truthfully.
        val len = file.length()
        if (len <= span.start) return
        val end = minOf(span.end, len - 1)
        val head = RangeServe.fileHead(
            span.start, end, span.total, mime,
            fullRequest && end == span.end,
        )
        val out = sock.getOutputStream()
        writeHead(out, head.code, head.message, head.headers)
        Log.i(TAG, "file ${file.name} ${head.code} ${head.headers["Content-Range"] ?: "full"} len=${head.headers["Content-Length"] ?: "?"}")
        RandomAccessFile(file, "r").use { raf ->
            raf.seek(span.start)
            val buf = ByteArray(CHUNK)
            var left = end - span.start + 1
            while (left > 0) {
                val n = raf.read(buf, 0, minOf(buf.size.toLong(), left).toInt())
                if (n < 0) break
                out.write(buf, 0, n)
                left -= n
            }
            out.flush()
        }
    }

    private fun writeHead(out: java.io.OutputStream, code: Int, msg: String, headers: Map<String, String>) {
        val sb = StringBuilder("HTTP/1.1 $code $msg\r\n")
        headers.forEach { (k, v) -> sb.append("$k: $v\r\n") }
        sb.append("\r\n")
        out.write(sb.toString().toByteArray())
        out.flush()
    }

    private fun reply(
        sock: Socket,
        code: Int,
        msg: String,
        headers: Map<String, String>,
        body: ByteArray?,
    ) {
        try {
            val out = sock.getOutputStream()
            writeHead(out, code, msg, headers)
            body?.let { out.write(it) }
            out.flush()
        } catch (_: Exception) {
        }
    }

    /** LRU eviction past the cap (pinned entries exempt unless all pinned). */
    private fun evictIfNeeded() {
        val entries = runBlocking(Dispatchers.IO) {
            runCatching { dao().allOrdered() }.getOrElse { emptyList() }
        }
        var used = entries.sumOf { it.bytes }
        if (used <= MAX_BYTES) return
        val dao = runBlocking(Dispatchers.IO) { dao() }
        fun drop(e: CacheEntry) {
            if (!serving.contains(e.trackId)) {
                runCatching { fileFor(e.trackId).delete() }
                runBlocking(Dispatchers.IO) { runCatching { dao.delete(e.trackId) } }
                used -= e.bytes
                Log.i(TAG, "evict ${e.trackId} (${e.bytes / 1024} KB)")
            }
        }
        entries.filter { it.playCount < PIN_PLAYS }.forEach { if (used > MAX_BYTES) drop(it) }
        entries.filter { it.playCount >= PIN_PLAYS }.forEach { if (used > MAX_BYTES) drop(it) }
    }
}
