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

    fun clearQueue() {
        update { s -> s.copy(queue = emptyList(), queueOriginal = emptyList()) }
        PlaybackStore.saveQueueSoon(emptyList())
    }

    fun removeAt(index: Int) {
        update { s -> s.copy(queue = s.queue.filterIndexed { i, _ -> i != index }) }
        PlaybackStore.saveQueueSoon(_state.value.queue)
    }

    /** Drag-reorder the queue (any music app semantics): move + persist. */
    fun moveQueue(from: Int, to: Int) {
        val q = _state.value.queue
        if (from !in q.indices || to !in q.indices || from == to) return
        val m = q.toMutableList()
        m.add(to, m.removeAt(from))
        update { s -> s.copy(queue = m) }
        PlaybackStore.saveQueueSoon(m)
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
        update { s -> s.copy(track = head, queue = s.queue.drop(1), isPlaying = true) }
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
        update { s ->
            s.copy(
                track = track,
                isPlaying = true,
                source = source.ifEmpty { s.source },
                audioTier = "",
            )
        }
        dispatchPlay(track)
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

    private fun playViaOpen(track: Track) {
        scope.launch {
            val res = withContext(Dispatchers.IO) { MusicRepository.resolveStream(track) }
            val json = res.getOrNull()
            val url = json?.optString("url", "")
            val svc = PlaybackService.instance
            if (!url.isNullOrEmpty() && svc != null) {
                update { s ->
                    s.copy(
                        audioTier = audioTier(
                            json?.optString("format", "") ?: "",
                            json?.optString("provider", "") ?: "",
                            json?.optString("quality", "") ?: "",
                        ),
                    )
                }
                svc.setPlayerVolume(_state.value.volume)
                svc.playUrl(url, track)
            } else {
                update { s -> s.copy(isPlaying = false) }
                Log.w(TAG, "play dropped: urlEmpty=${url.isNullOrEmpty()} svc=${svc != null}")
                ToastBus.fromBridge(res.exceptionOrNull() ?: Exception("no player"))
            }
        }
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
        scope.launch {
            val queue = PlaybackStore.loadQueue()
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
                    isPlaying = false,
                    positionMs = last?.positionMs ?: s.positionMs,
                    durationMs = track?.durationMs ?: s.durationMs,
                    source = last?.source ?: s.source,
                )
            }
            Log.i(TAG, "restored queue=${queue.size} last=${track?.name ?: "none"}")
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
