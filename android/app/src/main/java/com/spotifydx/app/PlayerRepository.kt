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
 * dedup-by-identifier ingestion, queue-first next-track, snapshot-restore
 * unshuffle, no-network queue screen — all carried over verbatim from the
 * current build. Open-engine transport (§9.6) resolves URLs through the core
 * and plays them on the platform stack here; SDK relay driving lands in
 * Phase 5.
 */
object PlayerRepository {
    private const val TAG = "SpotifyDxPlayer"
    enum class Repeat { OFF, CONTEXT, TRACK }

    data class State(
        val track: Track? = null,
        val queue: List<Track> = emptyList(),
        val queueOriginal: List<String> = emptyList(),
        /**
         * Played history, oldest→newest (Echo-style past songs; the queue
         * itself holds UPCOMING only). Jump-back truncates at the tapped
         * item — never duplicates, never disturbs the current track.
         */
        val history: List<Track> = emptyList(),
        val isPlaying: Boolean = false,
        val positionMs: Long = 0,
        val durationMs: Long = 0,
        val volume: Float = 0.8f,
        val shuffle: Boolean = false,
        val repeat: Repeat = Repeat.OFF,
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
        update { s -> s.copy(queue = emptyList(), queueOriginal = emptyList()) }
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
    private const val HISTORY_CAP = 50

    /** Records [track] as played (consecutive dupes skipped, oldest trimmed). */
    private fun pushHistory(track: Track?) {
        if (track == null || track.id.isEmpty()) return
        val h = _state.value.history
        if (h.lastOrNull()?.id == track.id) return
        val next = (h + track).takeLast(HISTORY_CAP)
        update { s -> s.copy(history = next) }
        PlaybackStore.saveHistorySoon(next)
    }

    fun clearHistory() {
        update { s -> s.copy(history = emptyList()) }
        PlaybackStore.saveHistorySoon(emptyList())
    }

    /**
     * Unified queue timeline (Echo single-list queue): past played + NOW
     * current + upcoming, in display order. One list to show, one list to
     * drag — never a pop-queue plus a detached history.
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
        val s = _state.value
        when (entry.kind) {
            RowKind.PAST -> {
                val h = s.history.toMutableList()
                val i = h.indexOfFirst { it.id == entry.track.id }
                if (i < 0) return
                h.removeAt(i)
                update { it.copy(history = h) }
                PlaybackStore.saveHistorySoon(h)
            }
            RowKind.NEXT -> {
                val q = s.queue.toMutableList()
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

    fun setShuffle(on: Boolean) = update { s ->
        if (on == s.shuffle) return@update s
        if (on) {
            val snap = s.queue.map { it.id }
            val shuffled = s.queue.shuffled()
            s.copy(shuffle = true, queueOriginal = snap, queue = shuffled)
        } else {
            val order = s.queueOriginal.withIndex().associate { it.value to it.index }
            val restored = s.queue.sortedBy { order[it.id] ?: Int.MAX_VALUE }
            s.copy(shuffle = false, queueOriginal = emptyList(), queue = restored)
        }
    }.also { PlaybackStore.saveQueueSoon(_state.value.queue) }

    fun setRepeat(mode: Repeat) = update { s -> s.copy(repeat = mode) }

    /** Queue-first next-track (prefers the queue head over device skip). */
    fun advance(): Track? {
        val head = _state.value.queue.firstOrNull() ?: return null
        pushHistory(_state.value.track)
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
        val prev = _state.value.track
        if (prev != null && prev.id != track.id) pushHistory(prev)
        startTrack(track, source)
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

    /** Shared track-launch tail (state flip + engine dispatch). */
    private fun startTrack(track: Track, source: String) {
        update { s ->
            s.copy(
                track = track,
                isPlaying = true,
                source = source.ifEmpty { s.source },
                audioTier = "",
                // Fresh track = fresh position; same track (replay after a
                // dead service) keeps the restored position for resume.
                positionMs = if (s.track?.id != track.id) 0 else s.positionMs,
            )
        }
        dispatchPlay(track)
    }

    /**
     * Echo `seekToDefaultPosition` semantics on our model: tapping any
     * timeline row makes it current while PRESERVING the full timeline on
     * both sides (windows behind become past, ahead stay upcoming) — never
     * a pop, never a jump. Position is dupe-safe (counted, not id-matched).
     */
    fun seekTimelinePosition(pos: Int) {
        val tl = timeline()
        val e = tl.getOrNull(pos) ?: return
        when (e.kind) {
            RowKind.NOW -> toggle()
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

    /** Tap an upcoming row: skipped rows (and the outgoing current) become past. */
    private fun seekUpcoming(idx: Int) {
        val s = _state.value
        val q = s.queue
        if (idx !in q.indices) return
        val skipped = q.subList(0, idx).toList()
        val track = q[idx]
        val rest = q.subList(idx + 1, q.size).toList()
        val hist = (s.history + listOfNotNull(s.track?.takeIf { it.id.isNotEmpty() }) + skipped)
            .takeLast(HISTORY_CAP)
        update { it.copy(track = track, queue = rest, history = hist, positionMs = 0) }
        PlaybackStore.saveHistorySoon(hist)
        PlaybackStore.saveQueueSoon(rest)
        startTrack(track, s.source)
    }

    /** Tap a past row: rows after it (and the outgoing current) return to upcoming. */
    private fun seekHistory(idx: Int) {
        val s = _state.value
        val h = s.history
        if (idx !in h.indices) return
        val track = h[idx]
        val upcoming = h.subList(idx + 1, h.size).toList() +
            listOfNotNull(s.track?.takeIf { it.id.isNotEmpty() }) + s.queue
        val hist = h.subList(0, idx).toList()
        update { it.copy(track = track, queue = upcoming, history = hist, positionMs = 0) }
        PlaybackStore.saveHistorySoon(hist)
        PlaybackStore.saveQueueSoon(upcoming)
        startTrack(track, "History")
    }

    private fun dispatchPlay(track: Track) {
        // New track = new last-played (position resets; timestamp = now).
        val s = _state.value
        PlaybackStore.saveLastSoon(track, 0, s.source)
        if (useSdkEngine()) {
            update { s -> s.copy(sdkActive = true, sdkDeviceId = null) }
            playViaSdk(track)
        } else {
            update { s -> s.copy(sdkActive = false, sdkDeviceId = null) }
            playViaOpen(track)
        }
    }

    private fun playViaOpen(track: Track, attempt: Int = 0) {
        scope.launch {
            val res = withContext(Dispatchers.IO) { MusicRepository.resolveStream(track) }
            val json = res.getOrNull()
            val url = json?.optString("url", "")
            var svc = PlaybackService.instance
            if (svc == null) {
                // Service dead/restarting is NOT a network problem: (re)start
                // it and give it one window instead of failing instantly.
                runCatching {
                    androidx.core.content.ContextCompat.startForegroundService(
                        AppState.ctx(), PlaybackService.intentOf(AppState.ctx()),
                    )
                }
                delay(500)
                svc = PlaybackService.instance
            }
            if (!url.isNullOrEmpty() && svc != null) {
                val format = json?.optString("format", "") ?: ""
                val provider = json?.optString("provider", "") ?: ""
                val quality = json?.optString("quality", "") ?: ""
                update { s ->
                    s.copy(audioTier = audioTier(format, provider, quality))
                }
                svc.setPlayerVolume(_state.value.volume)
                // Resume: the restored position belongs to THIS track only;
                // anything else (tap-while-resolving swapped tracks) starts
                // from the top. The quality tag keys the disk cache.
                val cur = _state.value
                val startMs =
                    if (cur.track?.id == track.id) cur.positionMs else 0
                svc.playUrl(url, track, startMs, "$provider/$format/$quality")
            } else {
                update { s -> s.copy(isPlaying = false) }
                val err = res.exceptionOrNull()
                val code = (err as? BridgeException)?.error
                    ?.let { it as? BridgeError.Core }?.code
                // Transient network failures get ONE backoff retry; region
                // blocks (NOT_FOUND) and session/premium errors never do.
                val transient = url.isNullOrEmpty() && (code == "NET" || code == "TIMEOUT")
                if (transient && attempt == 0) {
                    Log.i(TAG, "resolve transient ($code), retrying once after backoff")
                    delay(1500)
                    playViaOpen(track, 1)
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
            }
        }
    }

    /**
     * Re-resolve + replay after a transient media error (single attempt;
     * attempt=1 disables further resolve-retries — no loops). Same-track
     * resume keeps the position via the startMs path.
     */
    fun retryAfterError() {
        val t = _state.value.track ?: return
        playViaOpen(t, 1)
    }

    /** SDK path: ensure the hidden device, then Connect-play the URI on it.
     * Server-side start is confirmed by the `state` event stream. */
    private fun playViaSdk(track: Track) {
        val driver = sdkDriver
        if (driver == null) {
            Log.w(TAG, "SDK driver missing; falling back to open engine")
            update { s -> s.copy(sdkActive = false) }
            playViaOpen(track)
            return
        }
        scope.launch {
            if (!driver.ensure()) {
                update { s -> s.copy(isPlaying = false, sdkActive = false) }
                ToastBus.error("Spotify player unavailable — using the open engine.")
                playViaOpen(track)
                return@launch
            }
            val device = driver.awaitDevice() ?: run {
                update { s -> s.copy(isPlaying = false, sdkActive = false) }
                ToastBus.error("Spotify device not ready yet — using the open engine.")
                playViaOpen(track)
                return@launch
            }
            update { s -> s.copy(sdkDeviceId = device) }
            val uri = track.uri.ifEmpty { "spotify:track:${track.id}" }
            // Resume position belongs to THIS track only (same guard as open).
            val cur = _state.value
            val startMs = if (cur.track?.id == track.id) cur.positionMs else 0
            val res = withContext(Dispatchers.IO) { BridgeClient.sdkPlay(device, uri) }
            if (res.isFailure) {
                val code =
                    ((res.exceptionOrNull() as? BridgeException)?.error as? BridgeError.Core)?.code
                if (code == "PREMIUM_REQUIRED") {
                    // Forced-SDK on a free account: say so, then fall back.
                    ToastBus.fromBridge(res.exceptionOrNull() ?: Exception("premium required"))
                    update { s -> s.copy(sdkActive = false) }
                    playViaOpen(track)
                } else {
                    update { s -> s.copy(isPlaying = false) }
                    ToastBus.fromBridge(res.exceptionOrNull() ?: Exception("SDK play failed"))
                }
            } else if (startMs > 0) {
                withContext(Dispatchers.IO) { BridgeClient.sdkSeek(device, startMs) }
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
            // otherwise (re)resolve from the top (keeping the source).
            val resumed = svc?.resumePlayback() ?: false
            if (!resumed) {
                val track = s.track
                if (track != null) play(track, s.source)
            }
        }
    }

    fun nextTrack() {
        val next = advance() ?: return
        // Queue-first advance in both engines; dispatch picks the transport.
        dispatchPlay(next)
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
                    isPlaying = false,
                    positionMs = last?.positionMs ?: s.positionMs,
                    durationMs = track?.durationMs ?: s.durationMs,
                    source = last?.source ?: s.source,
                )
            }
            Log.i(TAG, "restored queue=${queue.size} history=${history.size} last=${track?.name ?: "none"}")
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
                    track = track ?: s.track,
                    isPlaying = playing,
                    positionMs = pos,
                    durationMs = dur,
                    sdkActive = true,
                )
            }
            PlaybackStore.saveLastSoon(
                track ?: _state.value.track, pos, _state.value.source,
            )
            publishSdkState()
        }
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
