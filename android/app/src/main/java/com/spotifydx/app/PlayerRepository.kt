package com.spotifydx.app

import android.util.Log
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * Playback state (§6, §9.1 migration). Owns the explicit local play queue:
 * dedup-by-identifier ingestion, queue-first next-track, no-network queue
 * screen — all carried over verbatim from the current build. Open-engine transport (§9.6) resolves URLs through the core
 * and plays them on the platform stack here; SDK relay driving lands in
 * Phase 5.
 */
object PlayerRepository {
    private const val TAG = "SpotifyDxPlayer"

    data class State(
        val track: Track? = null,
        val queue: List<Track> = emptyList(),
        /**
         * Session past window, oldest→newest (Echo-style past songs; the
         * queue itself holds UPCOMING only). Navigation state, not a
         * record: jump-back truncates at the tapped item, skipped rows
         * land here, drags re-split it by position — never duplicates,
         * never disturbs the current track. Drives the queue screen's past
         * section and prev/next walks ONLY (see [playLog] for the History
         * feature).
         */
        val history: List<Track> = emptyList(),
        /**
         * Append-only play log, oldest→newest: every track actually played,
         * exactly once. Written ONLY when a track is left going forward
         * (advance/next/context switch/tap-ahead); back-jumps, taps, and
         * drags never touch it. Drives the History feature (Home recent +
         * HistorySheet) and nothing else.
         */
        val playLog: List<Track> = emptyList(),
        val isPlaying: Boolean = false,
        val positionMs: Long = 0,
        val durationMs: Long = 0,
        val volume: Float = 0.8f,
        val transportReady: Boolean = false,
        /** True while the SDK device (not the platform player) owns audio. */
        val sdkActive: Boolean = false,
        val sdkDeviceId: String? = null,
        /** Play origin label ("Liked Songs", playlist/album name, "Queue"…). */
        val source: String = "",
        /** Display tier ("FLAC · Lossless", "320 kbps", "AAC"); "" = hidden. */
        val audioTier: String = "",
    )

    /** Tier label from resolve fields (honest: only what providers state). */
    fun audioTier(format: String, provider: String, quality: String): String = when {
        format == "flac" -> "FLAC · Lossless"
        provider == "saavn" && quality == "high" -> "320 kbps"
        provider == "saavn" -> "160 kbps"
        format == "mp3" -> "MP3"
        format == "aac" -> "AAC"
        format == "opus" -> "Opus"
        format == "ogg" -> "Ogg"
        else -> ""
    }

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val _state = MutableStateFlow(State())
    val state: StateFlow<State> = _state.asStateFlow()

    /** Compare-before-write: touching the store re-renders subscribers. */
    private fun update(f: (State) -> State) {
        val next = f(_state.value)
        if (next != _state.value) _state.value = next
    }

    // -- Local queue semantics (real now; no core needed) --------------------------
    // Every mutation persists the queue (debounced replace) so it survives
    // restarts; restore() rehydrates on boot.
    fun enqueue(track: Track) {
        update { s -> s.copy(queue = s.queue.filter { it.id != track.id } + track) }
        PlaybackStore.saveQueueSoon(_state.value.queue)
    }

    /**
     * Insert tracks to play next (head of upcoming, order preserved).
     * Dedupes by id like [enqueue] so repeats move instead of doubling.
     */
    fun playNext(tracks: List<Track>) {
        if (tracks.isEmpty()) return
        val ids = tracks.map { it.id }.toSet()
        update { s ->
            s.copy(
                queue = tracks.filter { it.playable } +
                    s.queue.filter { it.id !in ids },
            )
        }
        PlaybackStore.saveQueueSoon(_state.value.queue)
    }

    fun playNext(track: Track) = playNext(listOf(track))

    fun clearQueue() {
        update { s -> s.copy(queue = emptyList()) }
        PlaybackStore.saveQueueSoon(emptyList())
    }

    fun removeAt(index: Int) {
        update { s -> s.copy(queue = s.queue.filterIndexed { i, _ -> i != index }) }
        PlaybackStore.saveQueueSoon(_state.value.queue)
    }

    /**
     * Wholesale queue reorder (Echo commit-on-drop): the drag session owns
     * a visual index permutation and lands the fully-reordered list once.
     * Single emit + single persist — no mid-drag traffic, so the list can
     * never fight the gesture or snap back.
     */
    fun setQueueOrder(tracks: List<Track>) {
        update { s -> s.copy(queue = tracks.toList()) }
        PlaybackStore.saveQueueSoon(_state.value.queue)
    }

    /** Played-history cap (oldest trimmed, persisted debounced). */
    private const val HISTORY_CAP = 100

    /** Appends [track] to the session past window (consecutive dupes skipped). */
    private fun pushHistory(track: Track?) {
        if (track == null || track.id.isEmpty()) return
        val h = _state.value.history
        if (h.lastOrNull()?.id == track.id) return
        val next = (h + track).takeLast(HISTORY_CAP)
        update { s -> s.copy(history = next) }
        PlaybackStore.saveHistorySoon(next)
    }

    /**
     * Records [track] as most-recent in the durable play log: a replay
     * moves the existing entry to the tail instead of duplicating it.
     * Two coordinated writers, both idempotent by construction:
     * - start-log (primary): the playUrl/sdkPlay handoff, so a track
     *   appears the moment it starts sounding — not when the next one
     *   begins. Resolve failures never reach it (nothing unplayed logs).
     * - leave-log (backstop in [leaveForward]): guarantees every forward
     *   transition leaves the track logged even if a future starter
     *   bypassed the handoff points above.
     * The session past window intentionally keeps positional duplicates
     * (prev/next walks depend on them).
     */
    private fun logPlay(track: Track?) {
        if (track == null || track.id.isEmpty()) return
        val next = (_state.value.playLog.filter { it.id != track.id } + track)
            .takeLast(HISTORY_CAP)
        update { s -> s.copy(playLog = next) }
        PlaybackStore.savePlayLogSoon(next)
    }

    /**
     * Shared forward-leave tail: the outgoing current joins BOTH the
     * session past window (navigation) and the durable play log
     * (History feature). Every forward transition funnels here so the two
     * lists can never disagree about what was played.
     */
    private fun leaveForward(track: Track?) {
        pushHistory(track)
        logPlay(track)
    }

    fun clearHistory() {
        update { s -> s.copy(history = emptyList(), playLog = emptyList()) }
        PlaybackStore.saveHistorySoon(emptyList())
        PlaybackStore.savePlayLogSoon(emptyList())
    }

    /**
     * Unified queue timeline (Echo single-list queue): past played + NOW
     * current + upcoming, in display order. The past section renders the
     * session past window ([history]) — one list to show, one list to
     * drag. The durable play log ([playLog]) is a separate list for the
     * History feature and never renders here.
     */
    fun timeline(): List<QueueEntry> {
        val s = _state.value
        val rows = ArrayList<QueueEntry>(s.history.size + s.queue.size + 1)
        s.history.forEach { rows.add(QueueEntry(it, RowKind.PAST)) }
        s.track?.takeIf { it.playable }?.let { rows.add(QueueEntry(it, RowKind.NOW)) }
        s.queue.forEach { rows.add(QueueEntry(it, RowKind.NEXT)) }
        return rows
    }

    /**
     * Lands a drag-session timeline (Echo commit-on-drop). Membership
     * follows the entries' kinds with the NOW-kind position authoritative
     * (exactly one exists by construction — dupe track ids can't misfile,
     * unlike first-id-match); the current track object is NEVER touched —
     * reorder (even across the NOW row) cannot make the player jump
     * tracks, which is exactly Echo's cross-current bug, excluded by
     * construction.
     */
    fun commitTimeline(entries: List<QueueEntry>) {
        val tracks = entries.map { it.track }
        val nowIdx = entries.indexOfFirst { it.kind == RowKind.NOW }
        if (nowIdx < 0) {
            // No current row (shouldn't happen — NOW can't be swiped
            // away): fall back to kind membership.
            val history = entries.filter { it.kind == RowKind.PAST }.map { it.track }
                .takeLast(HISTORY_CAP)
            val queue = entries.filter { it.kind == RowKind.NEXT }.map { it.track }
            update { s -> s.copy(history = history, queue = queue) }
            PlaybackStore.saveHistorySoon(history)
            PlaybackStore.saveQueueSoon(queue)
            return
        }
        val history = tracks.subList(0, nowIdx).takeLast(HISTORY_CAP)
        val queue = tracks.subList(nowIdx + 1, tracks.size).toList()
        update { s -> s.copy(history = history, queue = queue) }
        PlaybackStore.saveHistorySoon(history)
        PlaybackStore.saveQueueSoon(queue)
    }

    /**
     * Drop commit for a drag gesture ([QueueDrag]): lands the user's
     * lift→hover move against LIVE repo state instead of the (possibly
     * stale) visual session. A track ending mid-drag — or any other queue
     * mutation — can therefore never corrupt the order or silently eat
     * the gesture. Returns true when a move was applied.
     */
    fun commitDrop(session: List<QueueEntry>, lift: Int, hover: Int): Boolean {
        if (lift !in session.indices || hover !in session.indices) return false
        val fresh = timeline()
        if (sameIdMultiset(session, fresh)) {
            // Nothing changed mid-drag: plain positional move on fresh
            // state (equivalent to the gesture, exact).
            val m = fresh.toMutableList()
            val e = m.removeAt(lift)
            m.add(hover.coerceIn(0, m.size), e)
            commitTimeline(m)
            return true
        }
        // Content changed mid-drag: anchor the drop to surviving neighbors
        // (dupe-safe via occurrence ranks), else abandon into a resync.
        // The dragged row itself sits at hover — neighbors are stable sides.
        // Membership splits ARITHMETICALLY around the live NOW position:
        // session kinds are stale here by definition, so they (and id
        // searches, which dupes defeat) are never consulted.
        val moved = session[lift]
        val anchors = listOfNotNull(
            session.getOrNull(hover - 1)?.let { it to true },
            session.getOrNull(hover + 1)?.let { it to false },
        )
        for ((anchor, placeAfter) in anchors) {
            val aIdx = if (placeAfter) hover - 1 else hover + 1
            val aRank = occurrenceRank(session, aIdx, anchor.track.id)
            val fAnchor = findOccurrence(fresh, anchor.track.id, aRank)
            if (fAnchor < 0) continue
            val mRank = occurrenceRank(session, lift, moved.track.id)
            val mIdx = findOccurrence(fresh, moved.track.id, mRank)
            if (mIdx < 0) return false
            val nfIdx = fresh.indexOfFirst { it.kind == RowKind.NOW }
            if (nfIdx < 0) {
                // No current track (never played): membership by kind —
                // the moved row inherits its anchor side's kind.
                val m = fresh.toMutableList()
                m.removeAt(mIdx)
                var ins = if (placeAfter) fAnchor + 1 else fAnchor
                if (mIdx < ins) ins--
                ins = ins.coerceIn(0, m.size)
                val anchorKind = m.getOrNull(if (placeAfter) ins - 1 else ins)?.kind
                    ?: moved.kind
                m.add(ins, moved.copy(kind = anchorKind))
                commitTimeline(m)
                return true
            }
            val tracks = fresh.map { it.track }.toMutableList()
            tracks.removeAt(mIdx)
            var ins = if (placeAfter) fAnchor + 1 else fAnchor
            if (mIdx < ins) ins--
            ins = ins.coerceIn(0, tracks.size)
            tracks.add(ins, moved.track)
            // The moved row lands at `ins`. NOW follows arithmetically:
            // it lives where it landed when IT moved, else shifts only if
            // the removal or insertion crossed it.
            val nowIdx = if (mIdx == nfIdx) {
                ins
            } else {
                var c = nfIdx
                if (mIdx < nfIdx) c--
                if (ins <= c) c++ else c
            }
            splitByIndex(tracks, nowIdx)
            return true
        }
        return false
    }

    /** 1-based occurrence rank of the id at pos within list. */
    private fun occurrenceRank(list: List<QueueEntry>, pos: Int, id: String): Int {
        if (pos !in list.indices) return 0
        return list.subList(0, pos + 1).count { it.track.id == id }
    }

    /** Index of the rank-th occurrence of id, or -1. */
    private fun findOccurrence(list: List<QueueEntry>, id: String, rank: Int): Int {
        if (rank <= 0) return -1
        var c = 0
        list.forEachIndexed { i, e ->
            if (e.track.id == id) {
                c++
                if (c == rank) return i
            }
        }
        return -1
    }

    private fun sameIdMultiset(a: List<QueueEntry>, b: List<QueueEntry>): Boolean {
        if (a.size != b.size) return false
        return a.map { it.track.id }.sorted() == b.map { it.track.id }.sorted()
    }

    /** Membership split at an explicit current index (dupe-proof). */
    private fun splitByIndex(tracks: List<Track>, nowIdx: Int) {
        val idx = nowIdx.coerceIn(0, tracks.size)
        val history = tracks.subList(0, idx).takeLast(HISTORY_CAP)
        val queue = tracks.subList((idx + 1).coerceAtMost(tracks.size), tracks.size).toList()
        update { s -> s.copy(history = history, queue = queue) }
        PlaybackStore.saveHistorySoon(history)
        PlaybackStore.saveQueueSoon(queue)
    }

    /** Swipe-remove support (Echo dismiss): excises one timeline entry. */
    fun deleteTimelineEntry(entry: QueueEntry) {
        deleteTimelineEntryAt(entry, -1)
    }

    /**
     * Positional swipe-remove: excises the exact swiped slot. The adapter
     * already removed positionally, so the repo must too — the old
     * first-id-match deleted the FIRST duplicate while the adapter removed
     * the swiped one, desyncing on the next resync (and mis-aiming Undo).
     * A negative position (or a timeline that moved under us) falls back
     * to the legacy id match rather than deleting the wrong row.
     */
    fun deleteTimelineEntryAt(entry: QueueEntry, globalPos: Int) {
        val s = _state.value
        if (globalPos >= 0) {
            val row = timeline().getOrNull(globalPos)
            if (row != null && row.kind == entry.kind && row.track.id == entry.track.id) {
                val nowCount = if (s.track?.playable == true) 1 else 0
                when (entry.kind) {
                    RowKind.PAST -> {
                        val h = s.history.toMutableList()
                        if (globalPos < h.size) {
                            h.removeAt(globalPos)
                            update { it.copy(history = h) }
                            PlaybackStore.saveHistorySoon(h)
                            return
                        }
                    }
                    RowKind.NEXT -> {
                        val q = s.queue.toMutableList()
                        val i = globalPos - s.history.size - nowCount
                        if (i in q.indices) {
                            q.removeAt(i)
                            update { it.copy(queue = q) }
                            PlaybackStore.saveQueueSoon(q)
                            return
                        }
                    }
                    RowKind.NOW -> return
                }
            }
        }
        deleteById(entry, s.history, s.queue)
    }

    /** Legacy id-match removal (fallback when the timeline moved under us). */
    private fun deleteById(entry: QueueEntry, history: List<Track>, queue: List<Track>) {
        when (entry.kind) {
            RowKind.PAST -> {
                val h = history.toMutableList()
                val i = h.indexOfFirst { it.id == entry.track.id }
                if (i < 0) return
                h.removeAt(i)
                update { it.copy(history = h) }
                PlaybackStore.saveHistorySoon(h)
            }
            RowKind.NEXT -> {
                val q = queue.toMutableList()
                val i = q.indexOfFirst { it.id == entry.track.id }
                if (i < 0) return
                q.removeAt(i)
                update { it.copy(queue = q) }
                PlaybackStore.saveQueueSoon(q)
            }
            RowKind.NOW -> return
        }
    }

    /** Undo support: splices an entry back at its timeline slot. */
    fun insertTimelineEntry(entry: QueueEntry, globalPos: Int) {
        val tl = timeline().toMutableList()
        tl.add(globalPos.coerceIn(0, tl.size), entry)
        commitTimeline(tl)
    }

    /**
     * Failure atomicity: every play mutates state optimistically (new
     * track shows instantly) and resolves async. If resolution fails, the
     * state must roll back to the pre-play snapshot — otherwise the UI
     * shows the failed track as paused while the service still holds the
     * old audio, and every later tap diverges further (the reported
     * "tapping pauses/plays at random"). Snapshots are taken by
     * [beginResolve] before any mutation; only the still-current
     * generation may restore (superseded failures touch nothing).
     */
    private var pendingRestore: State? = null
    private var resolveSeq = 0L

    /** Snapshots pre-play state; returns this play's generation. */
    private fun beginResolve(): Long {
        pendingRestore = _state.value
        resolveSeq++
        return resolveSeq
    }

    /**
     * Rolls back a failed play to its pre-play snapshot (track, queue,
     * history, log, position, playing flag) and re-persists it (a failed
     * play's debounced saves may already have landed). The service still
     * holds the old audio, so post-restore state and service agree: the
     * failed tap becomes a no-op plus a toast. (One accepted tradeoff: a
     * failed context play drops its just-enqueued rest from the queue —
     * re-tapping the album restores it; coherent state beats kept queue.)
     */
    private fun restoreFailure(prev: State) {
        Log.i(TAG, "play failed, restoring ${prev.track?.name ?: "none"}")
        pendingRestore = null
        update { prev }
        PlaybackStore.saveQueueSoon(prev.queue)
        PlaybackStore.saveHistorySoon(prev.history)
        PlaybackStore.savePlayLogSoon(prev.playLog)
        PlaybackStore.saveLastSoon(prev.track, prev.positionMs, prev.source)
    }

    /** Queue-first next-track (prefers the queue head over device skip). */
    fun advance(): Track? {
        val head = _state.value.queue.firstOrNull() ?: return null
        leaveForward(_state.value.track)
        update { s -> s.copy(track = head, queue = s.queue.drop(1), isPlaying = true, positionMs = 0) }
        PlaybackStore.saveQueueSoon(_state.value.queue)
        return head
    }

    // -- Transport: engine router (§9.1) + open engine (§9.6) --------------------
    /** Process-wide SDK driver host (owned by MainActivity, like the login
     * page host). Null until the activity attaches it. */
    var sdkDriver: SdkWebViewDriver? = null
        set(v) {
            field = v
            // Boot-time SDK failures are recovered by fallback and stay
            // log-only; only an established SDK session toasts errors.
            v?.onError = { m ->
                if (_state.value.sdkActive) ToastBus.error("Playback error: $m")
                else Log.w(TAG, "SDK boot failure (open fallback active): $m")
            }
        }

    /** Engine choice: explicit pref wins; auto follows the account tier
     * (SDK iff Premium — mirrors core `should_use_open_engine`). */
    fun useSdkEngine(): Boolean = when (SettingsStore.settings.value.engine) {
        "spotify-sdk" -> true
        "open" -> false
        else -> SessionRepository.snapshot().isPremium
    }

    fun play(track: Track, source: String = "") {
        val seq = beginResolve()
        val prev = _state.value.track
        if (prev != null && prev.id != track.id) leaveForward(prev)
        startTrack(track, source, seq)
    }

    /**
     * Play a context (album/playlist/artist top-tracks): [first] becomes
     * NOW (outgoing current → history via [play]) and [rest] lands between
     * it and the previous upcoming head, order preserved.
     */
    fun playContext(first: Track, rest: List<Track>, source: String = "") {
        play(first, source)
        playNext(rest)
    }

    /**
     * Resume after service death (crash/restart/system reclaim): re-resolve
     * and continue from the saved offset. Position is deliberately
     * untouched here — fresh plays always start at 0 (see startTrack).
     */
    private fun resumeResolved(track: Track, source: String) {
        val seq = beginResolve()
        update { s ->
            s.copy(
                track = track,
                isPlaying = true,
                source = source.ifEmpty { s.source },
                audioTier = "",
            )
        }
        // Resume keeps the saved offset: capture it synchronously here
        // (service is dead — no ticks coming; a live re-read after the
        // async resolve could pick up another track's ticks instead).
        dispatchPlay(track, seq, _state.value.positionMs)
    }

    /** Shared track-launch tail (state flip + engine dispatch). */
    private fun startTrack(track: Track, source: String, seq: Long) {
        update { s ->
            s.copy(
                track = track,
                isPlaying = true,
                source = source.ifEmpty { s.source },
                audioTier = "",
                // Explicit plays always start at 0 — even replays of the
                // loaded track (a boot-restored paused song otherwise
                // resumes from yesterday's saved offset). Pause/resume
                // keeps its offset via toggle/restore, never this path;
                // media-error recovery resumes via playViaOpen directly.
                positionMs = 0,
            )
        }
        // Fresh plays always start at 0: the offset travels as a parameter
        // (never re-read from live state after the async resolve — the old
        // audio's position ticks keep landing meanwhile and would resume
        // the NEW song from the OLD timestamp whenever it was playing).
        dispatchPlay(track, seq, 0)
    }

    /**
     * Echo `seekToDefaultPosition` semantics on our model: tapping any
     * timeline row makes it current while PRESERVING the full timeline on
     * both sides (windows behind become past, ahead stay upcoming) — never
     * a pop, never a jump. Position is dupe-safe (counted, not id-matched).
     * Only the outgoing current reaches the play log ([leaveForward] /
     * [logPlay]); positional reshuffles stay in the past window.
     */
    fun seekTimelinePosition(pos: Int) {
        val tl = timeline()
        val e = tl.getOrNull(pos) ?: return
        when (e.kind) {
            RowKind.NOW -> restartCurrent()
            RowKind.NEXT -> {
                val idx = tl.subList(0, pos).count { it.kind == RowKind.NEXT }
                seekUpcoming(idx)
            }
            RowKind.PAST -> {
                val idx = tl.subList(0, pos).count { it.kind == RowKind.PAST }
                seekHistory(idx)
            }
        }
    }

    /**
     * Tap the current row: restart it from the top and keep it playing.
     * Tapping always means "play this" — never a pause toggle.
     */
    fun restartCurrent() {
        val s = _state.value
        if (s.track == null) return
        seekTo(0)
        if (!s.isPlaying) toggle()
    }

    /**
     * Tap an upcoming row: the outgoing current joins the past window AND
     * the play log ([leaveForward]); the jumped rows were never played, so
     * they join the past window only (old queue behavior: the new current
     * is followed by the rows after it, nothing reshuffled) — never the
     * log.
     */
    private fun seekUpcoming(idx: Int) {
        val s = _state.value
        val q = s.queue
        if (idx !in q.indices) return
        val seq = beginResolve()
        val skipped = q.subList(0, idx).toList()
        val track = q[idx]
        val rest = q.subList(idx + 1, q.size).toList()
        val hist = (s.history + listOfNotNull(s.track?.takeIf { it.id.isNotEmpty() }) + skipped)
            .takeLast(HISTORY_CAP)
        logPlay(s.track)
        update { it.copy(track = track, queue = rest, history = hist, positionMs = 0) }
        PlaybackStore.saveHistorySoon(hist)
        PlaybackStore.saveQueueSoon(rest)
        startTrack(track, s.source, seq)
    }

    /** Tap a past row: rows after it (and the outgoing current) return to upcoming. */
    private fun seekHistory(idx: Int) {
        val s = _state.value
        val h = s.history
        if (idx !in h.indices) return
        val seq = beginResolve()
        val track = h[idx]
        val upcoming = h.subList(idx + 1, h.size).toList() +
            listOfNotNull(s.track?.takeIf { it.id.isNotEmpty() }) + s.queue
        val hist = h.subList(0, idx).toList()
        update { it.copy(track = track, queue = upcoming, history = hist, positionMs = 0) }
        PlaybackStore.saveHistorySoon(hist)
        PlaybackStore.saveQueueSoon(upcoming)
        startTrack(track, "History", seq)
    }

    private fun dispatchPlay(track: Track, seq: Long, startMs: Long) {
        // New track = new last-played (position resets; timestamp = now).
        val s = _state.value
        PlaybackStore.saveLastSoon(track, 0, s.source)
        if (useSdkEngine()) {
            update { s -> s.copy(sdkActive = true, sdkDeviceId = null) }
            playViaSdk(track, seq, startMs)
        } else {
            update { s -> s.copy(sdkActive = false, sdkDeviceId = null) }
            playViaOpen(track, 0, seq, startMs)
        }
    }

    private fun playViaOpen(track: Track, attempt: Int = 0, seq: Long, startMs: Long) {
        scope.launch {
            val res = withContext(Dispatchers.IO) { MusicRepository.resolveStream(track) }
            val json = res.getOrNull()
            val url = json?.optString("url", "")
            var svc = PlaybackService.instance
            if (svc == null) {
                // Service dead/restarting is NOT a network problem: (re)start
                // it instead of failing instantly. Plain startService —
                // NEVER startForegroundService here: that obligates the
                // service to foreground within seconds, and a slow start
                // (or a start with no playUrl following) then kills the
                // whole app (ForegroundServiceDidNotStartInTimeException).
                // playUrl foregrounds itself once it has audio to show.
                runCatching {
                    AppState.ctx().startService(PlaybackService.intentOf(AppState.ctx()))
                }
                // Poll for the instance: one fixed 500ms window missed slow
                // cold starts and stranded the play (or worse, a started
                // service with no foreground to follow).
                var waited = 0
                while (svc == null && waited < 10) {
                    delay(500)
                    svc = PlaybackService.instance
                    waited += 1
                }
            }
            if (!url.isNullOrEmpty() && svc != null) {
                if (seq == resolveSeq) pendingRestore = null
                val format = json?.optString("format", "") ?: ""
                val provider = json?.optString("provider", "") ?: ""
                val quality = json?.optString("quality", "") ?: ""
                update { s ->
                    s.copy(audioTier = audioTier(format, provider, quality))
                }
                svc.setPlayerVolume(_state.value.volume)
                // startMs was fixed at dispatch: re-reading positionMs here
                // would pick up the OLD audio's ticks (still playing until
                // this URL loads) whenever the old song wasn't paused.
                // The quality tag keys the disk cache.
                svc.playUrl(url, track, startMs, "$provider/$format/$quality")
                // Start-log: the track is handed to the service now, so it
                // joins history immediately (dedupe move-to-tail) instead
                // of waiting for the next track to begin.
                logPlay(track)
            } else {
                // Superseded plays (a newer tap started while this resolved)
                // touch nothing: state, toasts, and retries all belong to
                // the newer generation now.
                if (seq != resolveSeq) {
                    Log.i(TAG, "play superseded, ignoring failure for ${track.id}")
                    return@launch
                }
                val err = res.exceptionOrNull()
                val bridgeErr = (err as? BridgeException)?.error
                val code = (bridgeErr as? BridgeError.Core)?.code
                // Session heal (screens parity): resolution is token-free
                // now, but a stale core (or a future gate) can still answer
                // NEEDS_PAGE. Heal silently once and retry — never bounce
                // the user to a retry button for this (the pre-fix expiry
                // playback hell: every post-expiry tap failed instantly and
                // only a screen retry could heal it).
                if (url.isNullOrEmpty() && bridgeErr is BridgeError.NeedsPage && attempt == 0) {
                    Log.i(TAG, "resolve needs session, healing silently once")
                    if (SessionRefresher.refresh().isSuccess) {
                        if (seq != resolveSeq) return@launch
                        playViaOpen(track, 1, seq, startMs)
                        return@launch
                    }
                    // Heal failed: fall through to the failure handling
                    // below (rollback + toast), exactly as today.
                }
                // Transient network failures get ONE backoff retry; region
                // blocks (NOT_FOUND) and session/premium errors never do.
                val transient = url.isNullOrEmpty() && (code == "NET" || code == "TIMEOUT")
                if (transient && attempt == 0) {
                    Log.i(TAG, "resolve transient ($code), retrying once after backoff")
                    delay(1500)
                    if (seq != resolveSeq) return@launch
                    playViaOpen(track, 1, seq, startMs)
                    return@launch
                }
                when {
                    svc == null -> {
                        Log.w(TAG, "play dropped: service unavailable after restart")
                        ToastBus.error("Player unavailable — reopen the app")
                    }
                    code == "NOT_FOUND" -> {
                        Log.w(TAG, "play dropped: no playable source for ${track.id}")
                        ToastBus.error("Not available — nothing playable found")
                    }
                    transient -> {
                        Log.w(TAG, "play dropped: resolve failed twice ($code)")
                        ToastBus.error("Couldn't load song — check your connection")
                    }
                    else -> {
                        Log.w(TAG, "play dropped: urlEmpty=${url.isNullOrEmpty()} svc=${svc != null}")
                        ToastBus.fromBridge(err ?: Exception("no player"))
                    }
                }
                // Roll back to the pre-play snapshot (see beginResolve):
                // without this the UI shows the failed track as paused
                // while the service still holds the old audio, and every
                // later tap diverges further. Falls back to the old
                // isPlaying=false when no snapshot exists.
                pendingRestore?.let { restoreFailure(it) }
                    ?: update { s -> s.copy(isPlaying = false) }
            }
        }
    }

    /**
     * Re-resolve + replay after a transient media error (single attempt;
     * attempt=1 disables further resolve-retries — no loops). Same-track
     * resume keeps the position via the startMs path.
     */
    fun retryAfterError() {
        val s = _state.value
        val t = s.track ?: return
        // Same-track resume: live ticks ARE this track's own position, so
        // the current value is the right one (no track switch in flight —
        // a newer play would have superseded this generation).
        playViaOpen(t, 1, resolveSeq, s.positionMs)
    }

    /** SDK path: ensure the hidden device, then Connect-play the URI on it.
     * Server-side start is confirmed by the `state` event stream. */
    private fun playViaSdk(track: Track, seq: Long, startMs: Long) {
        val driver = sdkDriver
        if (driver == null) {
            Log.w(TAG, "SDK driver missing; falling back to open engine")
            update { s -> s.copy(sdkActive = false) }
            playViaOpen(track, 0, seq, startMs)
            return
        }
        scope.launch {
            if (!driver.ensure()) {
                update { s -> s.copy(isPlaying = false, sdkActive = false) }
                ToastBus.error("Spotify player unavailable — using the open engine.")
                playViaOpen(track, 0, seq, startMs)
                return@launch
            }
            val device = driver.awaitDevice() ?: run {
                update { s -> s.copy(isPlaying = false, sdkActive = false) }
                ToastBus.error("Spotify device not ready yet — using the open engine.")
                playViaOpen(track, 0, seq, startMs)
                return@launch
            }
            update { s -> s.copy(sdkDeviceId = device) }
            val uri = track.uri.ifEmpty { "spotify:track:${track.id}" }
            // startMs was fixed at dispatch (same tick-pollution rule as
            // open): a live re-read here would resume from the old audio's
            // position whenever it was still playing.
            val res = withContext(Dispatchers.IO) { BridgeClient.sdkPlay(device, uri) }
            if (res.isFailure) {
                val code =
                    ((res.exceptionOrNull() as? BridgeException)?.error as? BridgeError.Core)?.code
                if (code == "PREMIUM_REQUIRED") {
                    // Forced-SDK on a free account: say so, then fall back
                    // (the open handoff below logs on success).
                    ToastBus.fromBridge(res.exceptionOrNull() ?: Exception("premium required"))
                    update { s -> s.copy(sdkActive = false) }
                    playViaOpen(track, 0, seq, startMs)
                } else {
                    update { s -> s.copy(isPlaying = false) }
                    ToastBus.fromBridge(res.exceptionOrNull() ?: Exception("SDK play failed"))
                }
            } else {
                if (startMs > 0) {
                    withContext(Dispatchers.IO) { BridgeClient.sdkSeek(device, startMs) }
                }
                // Start-log (same rule as the open handoff above).
                logPlay(track)
            }
        }
    }

    fun toggle() {
        val s = _state.value
        if (s.sdkActive) {
            // Client-side relay (matches desktop): the server confirms via
            // the state stream; no Connect round-trip for pause/resume.
            if (s.isPlaying) {
                sdkDriver?.pause()
                update { it.copy(isPlaying = false) }
            } else {
                sdkDriver?.play()
                update { it.copy(isPlaying = true) }
            }
            publishSdkState()
            if (s.isPlaying) {
                PlaybackStore.saveLastSoon(s.track, _state.value.positionMs, s.source)
            }
            return
        }
        val svc = PlaybackService.instance
        if (s.isPlaying) {
            svc?.pausePlayback() ?: update { it.copy(isPlaying = false) }
            PlaybackStore.saveLastSoon(s.track, _state.value.positionMs, s.source)
        } else {
            // Resume in place when the service still holds the track;
            // otherwise (re)resolve from the saved offset (crash/restart
            // resume — the ONLY launch path that keeps position).
            val resumed = svc?.resumePlayback() ?: false
            if (!resumed) {
                val track = s.track
                if (track != null) resumeResolved(track, s.source)
            }
        }
    }

    fun nextTrack() {
        if (_state.value.queue.firstOrNull() == null) return
        val seq = beginResolve()
        val next = advance() ?: return
        // Queue-first advance in both engines; dispatch picks the transport.
        // Fresh advance always starts at 0 (see startTrack).
        dispatchPlay(next, seq, 0)
    }

    /**
     * Prev-button semantics (all transports share this): 4s+ into the song
     * restarts it; otherwise the outgoing current returns to upcoming and
     * the most recent past row becomes current. Empty past = restart.
     * Position comes from repo state (tick-mirrored, survives pause), not
     * the service — so paused presses behave identically. The log is never
     * truncated, so a back-jump leaves the target as history tail: step
     * past it when already sitting on it, or prev would stall on one song.
     */
    fun previousTrack() {
        val s = _state.value
        val cur = s.track ?: return
        if (s.positionMs >= 4_000 || s.history.isEmpty()) {
            seekTo(0)
            return
        }
        val h = s.history
        val idx = if (h.lastOrNull()?.id == cur.id) h.size - 2 else h.size - 1
        if (idx < 0) {
            seekTo(0)
            return
        }
        seekHistory(idx)
    }

    fun seekTo(ms: Long) {
        update { _state.value.copy(positionMs = ms) }
        val s = _state.value
        PlaybackStore.saveLastSoon(s.track, ms, s.source)
        if (s.sdkActive) {
            val device = s.sdkDeviceId
            if (device != null) {
                scope.launch {
                    withContext(Dispatchers.IO) { BridgeClient.sdkSeek(device, ms) }
                }
            }
        } else {
            PlaybackService.instance?.seekToMs(ms)
        }
    }

    fun setVolume(v: Float) {
        update { s -> s.copy(volume = v.coerceIn(0f, 1f)) }
        PlaybackService.instance?.setPlayerVolume(_state.value.volume)
        val s = _state.value
        if (s.sdkActive) {
            s.sdkDeviceId?.let { device ->
                val pct = (_state.value.volume * 100).toInt().coerceIn(0, 100)
                scope.launch {
                    withContext(Dispatchers.IO) { BridgeClient.sdkVolume(device, pct) }
                }
            }
        }
        persistVolume()
    }

    private var volumeJob: Job? = null

    /** Volume persists (debounced snapshot-then-save, §13). */
    private fun persistVolume() {
        volumeJob?.cancel()
        volumeJob = scope.launch {
            kotlinx.coroutines.delay(500)
            val cur = SettingsStore.settings.value
            SettingsStore.save(cur.copy(volume = _state.value.volume))
        }
    }

    /** Boot rehydrate: queue + last-played track/position/timestamp.
     * Always lands paused — restores state, never autoplay. */
    fun restore() {
        // Reopening into live background playback must not clobber it:
        // restore() unconditionally writes isPlaying=false + stale DB
        // position, which flipped the play/pause icons to "play" while
        // audio kept going. Live state already equals (or leads) storage.
        if (PlaybackService.instance?.isPlayingNow() == true) {
            Log.i(TAG, "restore skipped: service actively playing")
            return
        }
        scope.launch {
            val queue = PlaybackStore.loadQueue()
            val history = PlaybackStore.loadHistory()
            val playLog = PlaybackStore.loadPlayLog()
            val last = PlaybackStore.loadLast()
            val track = last?.trackJson?.let { raw ->
                runCatching {
                    Models.track(org.json.JSONObject(raw)).takeIf { it.playable }
                }.getOrNull()
            }
            update { s ->
                s.copy(
                    track = track ?: s.track,
                    queue = queue,
                    history = history,
                    playLog = playLog,
                    isPlaying = false,
                    positionMs = last?.positionMs ?: s.positionMs,
                    durationMs = track?.durationMs ?: s.durationMs,
                    source = last?.source ?: s.source,
                )
            }
            Log.i(TAG, "restored queue=${queue.size} history=${history.size} playlog=${playLog.size} last=${track?.name ?: "none"}")
        }
    }

    /** Position ticks mirrored from the service (event-driven, §9.5). */
    fun onPosition(positionMs: Long, durationMs: Long) = update { s ->
        if (s.positionMs == positionMs && s.durationMs == durationMs) s
        else s.copy(positionMs = positionMs, durationMs = durationMs)
    }

    fun onServiceState(playing: Boolean) = update { s ->
        if (s.isPlaying == playing) s else s.copy(isPlaying = playing)
    }

    // -- SDK state intake (§9.2): device + player_state_changed events ---------
    fun onSdkDevice(id: String) {
        update { s -> if (s.sdkActive) s.copy(sdkDeviceId = id) else s }
    }

    /** Apply an SDK `player_state_changed` payload (parsed by the core). */
    fun onSdkState(payloadJson: String) {
        if (!useSdkEngine() && !_state.value.sdkActive) return
        scope.launch {
            val st = withContext(Dispatchers.IO) {
                BridgeClient.sdkParseState(payloadJson).getOrNull()
            } ?: return@launch
            val playing = st.optBoolean("is_playing", false)
            val pos = st.optLong("position_ms", 0)
            val dur = st.optLong("duration_ms", 0)
            val track = st.optJSONObject("track")?.let { mapSdkTrack(it) }
            update { s ->
                s.copy(
                    track = mergeSdkTrack(s.track, track) ?: s.track,
                    isPlaying = playing,
                    positionMs = pos,
                    durationMs = dur,
                    sdkActive = true,
                )
            }
            PlaybackStore.saveLastSoon(
                _state.value.track, pos, _state.value.source,
            )
            publishSdkState()
        }
    }

    /**
     * SDK track merge: the SDK projection drops navigation ids
     * (`artistIds`, `albumId`, `addedAt`) and picks cover[0] instead of
     * widest — replacing the open-engine object with it stripped the
     * Artists/Album context-menu rows for SDK-driven tracks. When the SDK
     * reports the SAME track we hold, keep our full object (backfilling a
     * missing cover); a genuinely different id still replaces (the user
     * drove Spotify elsewhere), and a trackless payload keeps ours.
     */
    private fun mergeSdkTrack(cur: Track?, sdk: Track?): Track? = when {
        sdk == null -> cur
        cur != null && cur.id == sdk.id ->
            if (cur.coverUrl.isEmpty() && sdk.coverUrl.isNotEmpty()) {
                cur.copy(coverUrl = sdk.coverUrl)
            } else {
                cur
            }
        else -> sdk
    }

    private fun mapSdkTrack(o: org.json.JSONObject): Track? {
        val id = o.optString("id", "")
        val name = o.optString("name", "")
        if (id.isEmpty() || name.isEmpty()) return null
        val artists = o.optJSONArray("artists")?.let { arr ->
            List(arr.length()) { i -> arr.optJSONObject(i)?.optString("name", "") ?: "" }
        }?.filter { it.isNotEmpty() } ?: emptyList()
        val album = o.optJSONObject("album")
        val cover = album?.optJSONArray("images")?.optJSONObject(0)?.optString("url", "") ?: ""
        return Track(
            id = id,
            name = name,
            artists = artists,
            albumName = album?.optString("name", "") ?: "",
            coverUrl = cover,
            durationMs = o.optLong("duration_ms", 0),
            uri = o.optString("uri", ""),
            explicit = o.optBoolean("explicit", false),
        )
    }

    /** Mirror SDK-driven state into the notification + media session (the
     * service owns those surfaces on both engines). */
    private fun publishSdkState() {
        val s = _state.value
        val track = s.track ?: return
        PlaybackService.instance?.publishExternal(track, s.isPlaying, s.positionMs)
    }
}
