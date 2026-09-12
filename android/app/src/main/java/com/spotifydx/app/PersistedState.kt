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

    fun saveLastSoon(track: Track?, positionMs: Long, source: String = "") {
        scope.launch {
            withContext(Dispatchers.IO) {
                AppDb.get(AppState.ctx()).playback().save(
                    PlaybackStateRow(
                        trackJson = track?.let { Models.trackToJson(it).toString() },
                        positionMs = positionMs,
                        updatedAtMs = System.currentTimeMillis(),
                        source = source,
                    ),
                )
            }
        }
    }

    suspend fun loadLast(): PlaybackStateRow? =
        withContext(Dispatchers.IO) { AppDb.get(AppState.ctx()).playback().get() }

    suspend fun clearAll() {
        queueJob?.cancel()
        historyJob?.cancel()
        withContext(Dispatchers.IO) {
            val db = AppDb.get(AppState.ctx())
            db.queue().clear()
            db.history().clear()
            db.playback().clear()
        }
    }
}
