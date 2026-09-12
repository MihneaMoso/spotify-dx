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
 * Threading: one daemon acceptor + pooled connection handlers; per-key
 * locks serialize file access. The single player means one active stream;
 * the guards still hold under overlap.
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

    @Volatile
    private var port: Int = 0

    /** Fresh upstream URL per key (registered on every play start). */
    private val upstream = ConcurrentHashMap<String, String>()

    private val locks = ConcurrentHashMap<String, Any>()

    /** Keys with an active serving stream (exempt from eviction). */
    private val serving =
        Collections.newSetFromMap(ConcurrentHashMap<String, Boolean>())

    private fun lockFor(key: String): Any = locks.getOrPut(key) { Any() }

    private fun dir(): File = appCtx!!.filesDir.resolve(DIR).apply { mkdirs() }

    private fun fileFor(key: String): File = dir().resolve("$key.audio")

    private fun keyFor(track: Track): String {
        val clean = track.id.replace(Regex("[^A-Za-z0-9_-]"), "_").take(80)
        return clean.ifEmpty { "noid" }
    }

    private fun dao() = AppDb.get(appCtx!!).audioCache()

    /**
     * Playback source for a freshly resolved track. Complete files hit
     * disk directly; anything else streams through the proxy (which fills
     * the file as it serves). Never throws — falls back to proxy.
     */
    fun playbackSource(
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
            return Source.Proxy("http://127.0.0.1:$port/$liveKey")
        }
        val entry = try {
            runBlocking(Dispatchers.IO) { dao().entry(key) }
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
        Log.i(TAG, "MISS $key (proxying, have=${file.takeIf { it.isFile }?.length() ?: 0} B)")
        return Source.Proxy("http://127.0.0.1:$port/$key")
    }

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
            var rangeStart: Long? = null
            var rangeEnd: Long? = null
            while (true) {
                val line = input.readLine() ?: break
                if (line.isEmpty()) break
                if (line.startsWith("Range:", ignoreCase = true)) {
                    val spec = line.substringAfter(":").trim().removePrefix("bytes=")
                    val (a, b) = spec.split("-", limit = 2) + listOf("", "")
                    rangeStart = a.toLongOrNull()
                    rangeEnd = b.toLongOrNull()
                }
            }
            if (key.isEmpty()) {
                reply(s, 404, "Not Found", emptyMap(), null)
                return
            }
            try {
                serve(s, key, rangeStart, rangeEnd)
            } finally {
                // Ephemeral pass-through keys never repeat: drop them.
                if (key.endsWith("-live")) upstream.remove(key)
            }
        }
    }

    private fun serve(sock: Socket, key: String, reqStart: Long?, reqEnd: Long?) {
        val lock = lockFor(key)
        synchronized(lock) {
            serving.add(key)
            try {
                serveLocked(sock, key, reqStart, reqEnd)
            } catch (e: Exception) {
                Log.w(TAG, "serve $key: ${e.message}")
            } finally {
                serving.remove(key)
            }
        }
        scope.launch { evictIfNeeded() }
    }

    private fun serveLocked(sock: Socket, key: String, reqStart: Long?, reqEnd: Long?) {
        val file = fileFor(key)
        val have = if (file.isFile) file.length() else 0
        val meta = runBlocking(Dispatchers.IO) { dao().entry(key) }
        val total = meta?.totalBytes ?: -1

        // Pure-file fast path: requested span already on disk with known total.
        if (have > 0 && total > 0) {
            val start = reqStart ?: 0
            if (start < have) {
                val end = minOf(reqEnd ?: (total - 1), total - 1)
                sendFile(sock, file, start, end, total)
                return
            }
        } else if (have > 0 && reqStart != null && reqStart < have && total <= 0) {
            // Total unknown but the span is cached: serve to EOF, close-delimited.
            sendFile(sock, file, reqStart, have - 1, -1)
            return
        }

        // Need upstream: pass-through (ephemeral key) or stream-and-store.
        val url = upstream[key] ?: run {
            // No upstream registered: serve whatever is cached, else 502.
            if (have > 0) {
                val start = (reqStart ?: 0).coerceAtMost(have - 1)
                sendFile(sock, file, start, have - 1, total)
            } else {
                reply(sock, 502, "Bad Gateway", emptyMap(), null)
            }
            return
        }
        fetchAndTee(sock, key, url, file, have, reqStart, reqEnd, store = !key.endsWith("-live"))
    }

    /**
     * Fetches upstream (forwarding Range, resuming partial files) while
     * teeing bytes to disk + socket. Handles stale partials (server
     * ignores Range → restart from zero) and 416s.
     */
    private fun fetchAndTee(
        sock: Socket,
        key: String,
        url: String,
        file: File,
        have: Long,
        reqStart: Long?,
        reqEnd: Long?,
        store: Boolean,
    ) {
        // Resume only when the request continues exactly where we left off;
        // anything else restarts the file (stale partial from an expired URL
        // is unusable mid-file).
        val resumeAt = if (store && have > 0 && (reqStart == null || reqStart == 0L || reqStart == have)) {
            if (reqStart == null || reqStart == 0L) {
                if (have > 0) file.delete()
                0L
            } else {
                have
            }
        } else {
            if (store && have > 0 && reqStart != null && reqStart < have) {
                // Span already cached but total unknown (handled above when
                // total known) — fall through to plain upstream fetch.
            }
            if (store) file.delete()
            0L
        }
        val conn = (URL(url).openConnection() as HttpURLConnection).apply {
            connectTimeout = 15000
            readTimeout = 30000
            setRequestProperty("User-Agent", UA)
            if (resumeAt > 0) setRequestProperty("Range", "bytes=$resumeAt-")
            else if (reqStart != null) {
                val end = reqEnd?.let { "-$it" } ?: "-"
                setRequestProperty("Range", "bytes=$reqStart$end")
            }
            instanceFollowRedirects = true
        }
        val code = try {
            conn.connect()
            conn.responseCode
        } catch (e: Exception) {
            reply(sock, 502, "Bad Gateway", emptyMap(), null)
            return
        }
        if (code == 416) {
            reply(sock, 416, "Range Not Satisfiable", emptyMap(), null)
            return
        }
        if (code != 200 && code != 206) {
            reply(sock, 502, "Bad Gateway", emptyMap(), null)
            return
        }
        if (resumeAt > 0 && code == 200) {
            // Server ignored Range: the partial is unusable — restart clean.
            file.delete()
            streamBody(sock, key, conn, file, 0, store, restart = true)
            return
        }
        val total = parseTotal(conn, resumeAt)
        streamBody(sock, key, conn, file, resumeAt, store, restart = false, total = total)
    }

    private fun parseTotal(conn: HttpURLConnection, offset: Long): Long {
        conn.getHeaderField("Content-Range")?.let { cr ->
            // "bytes S-E/T"
            cr.substringAfter("/").trim().toLongOrNull()?.let { return it }
        }
        conn.getHeaderField("Content-Length")?.toLongOrNull()?.let { return it + offset }
        return -1
    }

    private fun streamBody(
        sock: Socket,
        key: String,
        conn: HttpURLConnection,
        file: File,
        offset: Long,
        store: Boolean,
        restart: Boolean,
        total: Long = -1,
    ) {
        // 206 when resuming or when the player sought; 200 for full fetches.
        val partial = offset > 0 || conn.responseCode == 206
        // Length unknown until headers arrive: omit Content-Length and let
        // connection-close delimit (MediaPlayer tolerates it mid-buffering).
        val headers = mutableMapOf(
            "Content-Type" to "application/octet-stream",
            "Accept-Ranges" to "bytes",
            "Connection" to "close",
        )
        val out = sock.getOutputStream()
        val raf = if (store) {
            if (restart) file.delete()
            RandomAccessFile(file, "rw").apply { if (!restart) seek(length()) }
        } else {
            null
        }
        try {
            // We learn the servable span only after headers: for 206 the
            // Content-Range pins it; for 200 we stream to EOF.
            if (partial) {
                val cr = conn.getHeaderField("Content-Range")
                val spanTotal = cr?.substringAfter("/")?.trim()?.toLongOrNull() ?: total
                if (spanTotal > 0) {
                    val end = conn.getHeaderField("Content-Range")
                        ?.substringBefore("/")?.substringAfter("-")?.trim()?.toLongOrNull()
                        ?: (spanTotal - 1)
                    headers["Content-Range"] = "bytes $offset-$end/$spanTotal"
                    headers["Content-Length"] = "${end - offset + 1}"
                }
                writeHead(out, 206, "Partial Content", headers)
            } else {
                conn.getHeaderField("Content-Length")?.let { headers["Content-Length"] = it }
                writeHead(out, 200, "OK", headers)
            }
            val buf = ByteArray(CHUNK)
            var written = 0L
            conn.inputStream.use { ins ->
                while (true) {
                    val n = ins.read(buf)
                    if (n < 0) break
                    out.write(buf, 0, n)
                    raf?.write(buf, 0, n)
                    written += n
                }
            }
            out.flush()
            if (store) {
                val finalLen = file.length()
                val finalTotal = total.takeIf { it > 0 }
                    ?: conn.getHeaderField("Content-Range")?.substringAfter("/")?.trim()?.toLongOrNull()
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

    private fun sendFile(sock: Socket, file: File, start: Long, end: Long, total: Long) {
        val out = sock.getOutputStream()
        val headers = mutableMapOf(
            "Content-Type" to "application/octet-stream",
            "Accept-Ranges" to "bytes",
            "Connection" to "close",
        )
        if (total > 0) {
            headers["Content-Range"] = "bytes $start-$end/$total"
            headers["Content-Length"] = "${end - start + 1}"
            writeHead(out, 206, "Partial Content", headers)
        } else {
            writeHead(out, 200, "OK", headers)
        }
        RandomAccessFile(file, "r").use { raf ->
            raf.seek(start)
            val buf = ByteArray(CHUNK)
            var left = end - start + 1
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
