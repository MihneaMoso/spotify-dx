package com.spotifydx.app

import android.content.Context
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * Application context holder for process-wide repositories that have no
 * Activity in scope (set once from MainActivity; mirrors the
 * PlaybackService.instance pattern family).
 */
object AppState {
    private var appCtx: Context? = null

    fun init(ctx: Context) {
        if (appCtx == null) appCtx = ctx.applicationContext
    }

    fun ctx(): Context = requireNotNull(appCtx) { "AppState.init(applicationContext) missing" }
}

/**
 * Search history (Room `search_history`, LRU-capped at [AppDb.HISTORY_KEEP]).
 * Queries shorter than 2 chars are ignored (prefix typing noise).
 */
object SearchHistory {
    suspend fun record(raw: String) {
        val q = raw.trim()
        if (q.length < 2) return
        withContext(Dispatchers.IO) {
            val dao = AppDb.get(AppState.ctx()).search()
            dao.upsert(SearchEntry(q, System.currentTimeMillis(), (dao.count(q) ?: 0) + 1))
            dao.trim(AppDb.HISTORY_KEEP)
        }
    }

    suspend fun recent(limit: Int = 8): List<String> =
        withContext(Dispatchers.IO) {
            AppDb.get(AppState.ctx()).search().recent(limit).map { it.query }
        }

    suspend fun clear() {
        withContext(Dispatchers.IO) { AppDb.get(AppState.ctx()).search().clear() }
    }
}

/**
 * Queue + last-played persistence (Room `queue_items` / `playback_state`).
 * Queue writes are debounced (rapid enqueue/shuffle bursts collapse into one
 * transaction); last-played is written on track change / pause / seek —
 * never on the 250ms position ticker.
 */
object PlaybackStore {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private var queueJob: Job? = null
    private var historyJob: Job? = null
    private var playLogJob: Job? = null
    private var lastJob: Job? = null
    /**
     * Last-played write generation: every save takes a number, every wipe
     * burns the sequence. A save whose number is stale at write time is
     * dropped — without this, a pause/seek → immediate-logout ordering
     * let the IO land AFTER the wipe and resurrected the previous
     * account's track for the next sign-in. Atomic: wipes may arrive
     * from any thread.
     */
    private val lastGen = java.util.concurrent.atomic.AtomicLong(0)

    fun saveQueueSoon(tracks: List<Track>) {
        queueJob?.cancel()
        queueJob = scope.launch {
            delay(400)
            withContext(Dispatchers.IO) {
                val dao = AppDb.get(AppState.ctx()).queue()
                dao.replace(
                    tracks.mapIndexed { i, t ->
                        QueueItem(i, t.id, Models.trackToJson(t).toString())
                    },
                )
            }
        }
    }

    /** Debounced played-history persist (mirrors the queue path). */
    fun saveHistorySoon(tracks: List<Track>) {
        historyJob?.cancel()
        historyJob = scope.launch {
            delay(400)
            withContext(Dispatchers.IO) {
                val dao = AppDb.get(AppState.ctx()).history()
                dao.replace(
                    tracks.mapIndexed { i, t ->
                        HistoryItem(i, t.id, Models.trackToJson(t).toString())
                    },
                )
            }
        }
    }

    /** Debounced play-log persist (durable History feature). */
    fun savePlayLogSoon(tracks: List<Track>) {
        playLogJob?.cancel()
        playLogJob = scope.launch {
            delay(400)
            withContext(Dispatchers.IO) {
                val dao = AppDb.get(AppState.ctx()).playLog()
                dao.replace(
                    tracks.mapIndexed { i, t ->
                        PlayLogItem(i, t.id, Models.trackToJson(t).toString())
                    },
                )
            }
        }
    }

    suspend fun loadQueue(): List<Track> =
        withContext(Dispatchers.IO) {
            AppDb.get(AppState.ctx()).queue().all().mapNotNull { item ->
                runCatching {
                    Models.track(org.json.JSONObject(item.trackJson)).takeIf { it.playable }
                }.getOrNull()
            }
        }

    suspend fun loadHistory(): List<Track> =
        withContext(Dispatchers.IO) {
            AppDb.get(AppState.ctx()).history().all().mapNotNull { item ->
                runCatching {
                    Models.track(org.json.JSONObject(item.trackJson)).takeIf { it.playable }
                }.getOrNull()
            }
        }

    suspend fun loadPlayLog(): List<Track> =
        withContext(Dispatchers.IO) {
            AppDb.get(AppState.ctx()).playLog().all().mapNotNull { item ->
                runCatching {
                    Models.track(org.json.JSONObject(item.trackJson)).takeIf { it.playable }
                }.getOrNull()
            }
        }

    /**
     * True when any playback state survived on disk (last-played row,
     * queue, past window, or play log). A completely blank store on cold
     * start proves a fresh device — no valid session can predate it — so
     * the boot flow may route to GATE without waiting out a slow core. (A
     * logged-in user who never played has no rows either, but then the
     * core answers fast and the settled path wins before this fallback
     * matters.)
     */
    suspend fun hasPersistedState(): Boolean = withContext(Dispatchers.IO) {
        val db = AppDb.get(AppState.ctx())
        runCatching { db.playback().get() != null }.getOrDefault(false) ||
            runCatching { db.queue().all().isNotEmpty() }.getOrDefault(false) ||
            runCatching { db.history().all().isNotEmpty() }.getOrDefault(false) ||
            runCatching { db.playLog().all().isNotEmpty() }.getOrDefault(false)
    }

    fun saveLastSoon(track: Track?, positionMs: Long, source: String = "") {
        lastJob?.cancel()
        lastJob = scope.launch {
            val gen = lastGen.incrementAndGet()
            withContext(Dispatchers.IO) {
                // Stale (superseded or wiped while in flight) → drop, never
                // write: the final row must always be the latest intent.
                if (gen != lastGen.get()) return@withContext
                val dao = AppDb.get(AppState.ctx()).playback()
                dao.save(
                    PlaybackStateRow(
                        trackJson = track?.let { Models.trackToJson(it).toString() },
                        positionMs = positionMs,
                        updatedAtMs = System.currentTimeMillis(),
                        source = source,
                    ),
                )
                // A wipe that landed mid-write still wins: its generation
                // is newer, so remove what just landed (cancellation is
                // cooperative — the Room call above is not cancellable).
                if (gen != lastGen.get()) dao.clear()
            }
        }
    }

    suspend fun loadLast(): PlaybackStateRow? =
        withContext(Dispatchers.IO) { AppDb.get(AppState.ctx()).playback().get() }

    suspend fun clearAll() {
        queueJob?.cancel()
        historyJob?.cancel()
        playLogJob?.cancel()
        lastJob?.cancel()
        lastGen.incrementAndGet()
        withContext(Dispatchers.IO) {
            val db = AppDb.get(AppState.ctx())
            db.queue().clear()
            db.history().clear()
            db.playLog().clear()
            db.playback().clear()
        }
    }

    /**
     * Session-scoped wipe (logout / account switch): queue, last-played,
     * and the past window belong to the account's listening session, but
     * the play log is device-level past songs (Echo parity) and survives
     * — it is also the reinstall-restore payload via Auto Backup, so
     * wiping it here would defeat that durability.
     */
    suspend fun clearSession() {
        queueJob?.cancel()
        historyJob?.cancel()
        lastJob?.cancel()
        lastGen.incrementAndGet()
        withContext(Dispatchers.IO) {
            val db = AppDb.get(AppState.ctx())
            db.queue().clear()
            db.history().clear()
            db.playback().clear()
        }
    }
}
