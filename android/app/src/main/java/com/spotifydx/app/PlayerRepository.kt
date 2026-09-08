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
        val isPlaying: Boolean = false,
        val positionMs: Long = 0,
        val durationMs: Long = 0,
        val volume: Float = 0.8f,
        val shuffle: Boolean = false,
        val repeat: Repeat = Repeat.OFF,
        val transportReady: Boolean = false,
    )

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val _state = MutableStateFlow(State())
    val state: StateFlow<State> = _state.asStateFlow()

    /** Compare-before-write: touching the store re-renders subscribers. */
    private fun update(f: (State) -> State) {
        val next = f(_state.value)
        if (next != _state.value) _state.value = next
    }

    // -- Local queue semantics (real now; no core needed) --------------------------
    fun enqueue(track: Track) = update { s ->
        s.copy(queue = s.queue.filter { it.id != track.id } + track)
    }

    fun clearQueue() = update { s -> s.copy(queue = emptyList(), queueOriginal = emptyList()) }

    fun removeAt(index: Int) = update { s ->
        s.copy(queue = s.queue.filterIndexed { i, _ -> i != index })
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
    }

    fun setRepeat(mode: Repeat) = update { s -> s.copy(repeat = mode) }

    /** Queue-first next-track (prefers the queue head over device skip). */
    fun advance(): Track? {
        val head = _state.value.queue.firstOrNull() ?: return null
        update { s -> s.copy(track = head, queue = s.queue.drop(1), isPlaying = true) }
        return head
    }

    // -- Transport: open engine (§9.6) — resolve URL, platform player plays -----
    // -- SDK transport (relay driving) lands in Phase 5; these stubs stay. --
    fun play(track: Track) {
        update { s -> s.copy(track = track, isPlaying = true) }
        scope.launch {
            val res = withContext(Dispatchers.IO) { MusicRepository.resolveStream(track) }
            val url = res.getOrNull()?.optString("url", "")
            val svc = PlaybackService.instance
            if (!url.isNullOrEmpty() && svc != null) {
                svc.setPlayerVolume(_state.value.volume)
                svc.playUrl(url, track)
            } else {
                update { s -> s.copy(isPlaying = false) }
                Log.w(TAG, "play dropped: urlEmpty=${url.isNullOrEmpty()} svc=${svc != null}")
                ToastBus.fromBridge(res.exceptionOrNull() ?: Exception("no player"))
            }
        }
    }

    fun toggle() {
        val s = _state.value
        val svc = PlaybackService.instance
        if (s.isPlaying) {
            svc?.pausePlayback() ?: update { it.copy(isPlaying = false) }
        } else {
            // Resume in place when the service still holds the track;
            // otherwise (re)resolve from the top.
            val resumed = svc?.resumePlayback() ?: false
            if (!resumed) {
                val track = s.track
                if (track != null) play(track)
            }
        }
    }

    fun nextTrack() {
        val next = advance() ?: return
        play(next)
    }

    fun seekTo(ms: Long) {
        update { _state.value.copy(positionMs = ms) }
        PlaybackService.instance?.seekToMs(ms)
    }

    fun setVolume(v: Float) {
        update { s -> s.copy(volume = v.coerceIn(0f, 1f)) }
        PlaybackService.instance?.setPlayerVolume(_state.value.volume)
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

    /** Position ticks mirrored from the service (event-driven, §9.5). */
    fun onPosition(positionMs: Long, durationMs: Long) = update { s ->
        if (s.positionMs == positionMs && s.durationMs == durationMs) s
        else s.copy(positionMs = positionMs, durationMs = durationMs)
    }

    fun onServiceState(playing: Boolean) = update { s ->
        if (s.isPlaying == playing) s else s.copy(isPlaying = playing)
    }
}
