package com.spotifydx.app

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.media.MediaPlayer
import android.media.session.MediaSession
import android.media.session.PlaybackState
import android.os.Binder
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import android.util.Log

/**
 * Foreground playback service + media session (§9.6–9.7 migration).
 *
 * Recommended architecture: the core resolves and caches the stream URL
 * ([CoreBridge.resolveStream], Phase 4); Kotlin hands it to the platform
 * player here, which owns output, audio focus, notifications, and
 * lock-screen controls. Position ticks mirror into [PlayerRepository] with
 * compare-before-write (§9.5 — no polling ticker on either side: the
 * 250 ms publisher below runs only while audio actually plays).
 *
 * Requires: foreground service (persistent notification, audio-focus
 * handling, interruption reconciliation into player state) + media-session
 * integration (lock-screen transport, headset/Bluetooth controls, system
 * volume mapping). No interface rebuild on audio-focus changes — state
 * reconciliation only.
 */
class PlaybackService : Service(),
    MediaPlayer.OnPreparedListener,
    MediaPlayer.OnCompletionListener,
    MediaPlayer.OnErrorListener,
    AudioManager.OnAudioFocusChangeListener {

    companion object {
        const val TAG = "SpotifyDxPlayback"
        const val CHANNEL_ID = "spotifydx_playback"
        const val NOTIFICATION_ID = 1

        const val ACTION_TOGGLE = "com.spotifydx.app.TOGGLE"
        const val ACTION_NEXT = "com.spotifydx.app.NEXT"
        const val ACTION_PREV = "com.spotifydx.app.PREV"

        fun intentOf(ctx: Context): Intent = Intent(ctx, PlaybackService::class.java)

        /** Process-wide service handle (service outlives every Activity). */
        var instance: PlaybackService? = null
            private set
    }

    private val binder = LocalBinder()
    private var player: MediaPlayer? = null
    private var session: MediaSession? = null
    private var audioManager: AudioManager? = null
    private var focusRequest: AudioFocusRequest? = null
    private var ticker: Thread? = null

    inner class LocalBinder : Binder() {
        fun service(): PlaybackService = this@PlaybackService
    }

    override fun onBind(intent: Intent): IBinder = binder

    override fun onCreate() {
        super.onCreate()
        instance = this
        ensureChannel()
        audioManager = getSystemService(Context.AUDIO_SERVICE) as AudioManager
        session = MediaSession(this, "SpotifyDx").apply {
            setCallback(object : MediaSession.Callback() {
                override fun onPlay() = toggleFromExternal(wantPlaying = true)
                override fun onPause() = toggleFromExternal(wantPlaying = false)
                override fun onSkipToNext() = nextFromExternal()
                override fun onSkipToPrevious() = prevFromExternal()
                override fun onSeekTo(pos: Long) = PlayerRepository.seekTo(pos)
            })
            isActive = true
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_TOGGLE -> PlayerRepository.toggle()
            ACTION_NEXT -> nextFromExternal()
            ACTION_PREV -> prevFromExternal()
        }
        // Started once from MainActivity; START_STICKY keeps audio alive
        // across UI recreation (core outlives the interface, §12.5).
        return START_STICKY
    }

    /** Play a resolved stream URL (Phase 4 hands these over per track). */
    fun playUrl(url: String, track: Track) {
        if (!requestFocus()) {
            Log.w(TAG, "audio focus denied; reconciling to paused")
            PlayerRepository.onServiceState(false)
            return
        }
        releasePlayer()
        val mp = MediaPlayer().apply {
            setAudioAttributes(
                AudioAttributes.Builder()
                    .setUsage(AudioAttributes.USAGE_MEDIA)
                    .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
                    .build(),
            )
            setWakeMode(applicationContext, PowerManager.PARTIAL_WAKE_LOCK)
            setDataSource(url)
            setOnPreparedListener(this@PlaybackService)
            setOnCompletionListener(this@PlaybackService)
            setOnErrorListener(this@PlaybackService)
            prepareAsync()
        }
        player = mp
        startForeground(NOTIFICATION_ID, buildNotification(track, playing = true))
        updateSession(PlaybackState.STATE_BUFFERING, 0, track)
    }

    fun pausePlayback() {
        player?.takeIf { it.isPlaying }?.pause()
        stopTicker()
        PlayerRepository.onServiceState(false)
        PlayerRepository.state.value.track?.let {
            updateNotification(it, playing = false)
            updateSession(PlaybackState.STATE_PAUSED, currentMs(), it)
        }
    }

    fun resumePlayback(): Boolean {
        val mp = player ?: return false
        if (!requestFocus()) return false
        mp.start()
        startTicker()
        PlayerRepository.onServiceState(true)
        PlayerRepository.state.value.track?.let {
            updateNotification(it, playing = true)
            updateSession(PlaybackState.STATE_PLAYING, currentMs(), it)
        }
        return true
    }

    fun seekToMs(ms: Long) {
        val mp = player ?: return
        runCatching { mp.seekTo(ms.toInt()) }
        PlayerRepository.state.value.track?.let {
            updateSession(
                if (mp.isPlaying) PlaybackState.STATE_PLAYING else PlaybackState.STATE_PAUSED,
                currentMs(), it,
            )
        }
    }

    fun setPlayerVolume(v: Float) {
        player?.setVolume(v, v)
    }

    fun stopPlayback() {
        stopTicker()
        releasePlayer()
        abandonFocus()
        PlayerRepository.onServiceState(false)
        stopForeground(STOP_FOREGROUND_REMOVE)
    }

    /** Publish externally-driven (SDK-path) state to the notification +
     * media session. The service owns those surfaces on both engines; only
     * the audio source differs. */
    fun publishExternal(track: Track, playing: Boolean, posMs: Long) {
        updateNotification(track, playing)
        updateSession(
            if (playing) PlaybackState.STATE_PLAYING else PlaybackState.STATE_PAUSED,
            posMs, track,
        )
    }

    // -- MediaPlayer callbacks ----------------------------------------------------
    override fun onPrepared(mp: MediaPlayer) {
        mp.start()
        PlayerRepository.onServiceState(true)
        PlayerRepository.onPosition(0, mp.duration.toLong())
        startTicker()
        PlayerRepository.state.value.track?.let {
            updateNotification(it, playing = true)
            updateSession(PlaybackState.STATE_PLAYING, 0, it)
        }
    }

    override fun onCompletion(mp: MediaPlayer) {
        // Gapless-feel advance: the next queue head resolves + starts.
        // Empty queue ends playback (repeat-track replay is hardening work).
        if (PlayerRepository.state.value.queue.isEmpty()) {
            stopPlayback()
            return
        }
        PlayerRepository.nextTrack()
    }

    override fun onError(mp: MediaPlayer, what: Int, extra: Int): Boolean {
        Log.w(TAG, "mediaplayer error what=$what extra=$extra")
        ToastBus.error("Playback failed (error $what)")
        PlayerRepository.onServiceState(false)
        return true
    }

    // -- Position publisher: runs ONLY while playing (§9.5, §14.1) -----------------
    private fun startTicker() {
        stopTicker()
        ticker = Thread({
            try {
                while (player?.isPlaying == true) {
                    val mp = player ?: break
                    PlayerRepository.onPosition(mp.currentPosition.toLong(), mp.duration.toLong())
                    Thread.sleep(250)
                }
            } catch (_: InterruptedException) {
                // shutdown path
            }
        }, "spotifydx-position").also { it.isDaemon = true; it.start() }
    }

    private fun stopTicker() {
        ticker?.interrupt()
        ticker = null
    }

    private fun currentMs(): Long =
        runCatching { player?.currentPosition?.toLong() ?: 0 }.getOrDefault(0)

    // -- External transport (lock-screen / headset / notification) ------------------
    private fun toggleFromExternal(wantPlaying: Boolean) {
        if (wantPlaying == PlayerRepository.state.value.isPlaying) return
        PlayerRepository.toggle()
    }

    private fun nextFromExternal() {
        PlayerRepository.nextTrack()
    }

    private fun prevFromExternal() {
        // Restart-first semantics live in the ViewModel; the service only
        // reconciles what the repository already decided.
        PlayerRepository.seekTo(0)
    }

    // -- Audio focus: reconciliation only, never an interface rebuild ---------------
    private fun requestFocus(): Boolean {
        val am = audioManager ?: return true
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val req = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN)
                .setAudioAttributes(
                    AudioAttributes.Builder()
                        .setUsage(AudioAttributes.USAGE_MEDIA)
                        .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
                        .build(),
                )
                .setOnAudioFocusChangeListener(this)
                .build()
            focusRequest = req
            am.requestAudioFocus(req) == AudioManager.AUDIOFOCUS_REQUEST_GRANTED
        } else {
            @Suppress("DEPRECATION")
            am.requestAudioFocus(
                this,
                AudioManager.STREAM_MUSIC,
                AudioManager.AUDIOFOCUS_GAIN,
            ) == AudioManager.AUDIOFOCUS_REQUEST_GRANTED
        }
    }

    private fun abandonFocus() {
        val am = audioManager ?: return
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            focusRequest?.let { am.abandonAudioFocusRequest(it) }
        } else {
            @Suppress("DEPRECATION")
            am.abandonAudioFocus(this)
        }
        focusRequest = null
    }

    override fun onAudioFocusChange(focusChange: Int) {
        when (focusChange) {
            AudioManager.AUDIOFOCUS_LOSS,
            AudioManager.AUDIOFOCUS_LOSS_TRANSIENT -> pausePlayback()
            AudioManager.AUDIOFOCUS_LOSS_TRANSIENT_CAN_DUCK -> {
                player?.setVolume(0.2f, 0.2f)
            }
            AudioManager.AUDIOFOCUS_GAIN -> {
                player?.setVolume(1f, 1f)
            }
        }
    }

    // -- Notification + session -------------------------------------------------------
    private fun ensureChannel() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        val nm = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        nm.createNotificationChannel(
            NotificationChannel(CHANNEL_ID, "Playback", NotificationManager.IMPORTANCE_LOW),
        )
    }

    private fun contentIntent(): PendingIntent {
        val i = packageManager.getLaunchIntentForPackage(packageName) ?: Intent(this, MainActivity::class.java)
        return PendingIntent.getActivity(
            this, 0, i,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
    }

    private fun actionIntent(action: String, code: Int): PendingIntent =
        PendingIntent.getService(
            this, code, Intent(this, PlaybackService::class.java).setAction(action),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )

    @Suppress("DEPRECATION")
    private fun buildNotification(track: Track, playing: Boolean): Notification {
        val style = Notification.MediaStyle()
            .setMediaSession(session?.sessionToken)
            .setShowActionsInCompactView(0, 1, 2)
        val builder = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            Notification.Builder(this, CHANNEL_ID)
        } else {
            @Suppress("DEPRECATION")
            Notification.Builder(this)
        }
        return builder
            .setContentTitle(track.name.ifEmpty { "Spotify DX" })
            .setContentText(track.artistNames.ifEmpty { "Ready to play" })
            .setSmallIcon(android.R.drawable.ic_media_play)
            .setContentIntent(contentIntent())
            .setStyle(style)
            .addAction(android.R.drawable.ic_media_previous, "Previous", actionIntent(ACTION_PREV, 1))
            .addAction(
                if (playing) android.R.drawable.ic_media_pause else android.R.drawable.ic_media_play,
                if (playing) "Pause" else "Play",
                actionIntent(ACTION_TOGGLE, 2),
            )
            .addAction(android.R.drawable.ic_media_next, "Next", actionIntent(ACTION_NEXT, 3))
            .build()
    }

    private fun updateNotification(track: Track, playing: Boolean) {
        val nm = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        nm.notify(NOTIFICATION_ID, buildNotification(track, playing))
    }

    private fun updateSession(state: Int, pos: Long, track: Track) {
        session?.setPlaybackState(
            PlaybackState.Builder()
                .setActions(
                    PlaybackState.ACTION_PLAY or PlaybackState.ACTION_PAUSE or
                        PlaybackState.ACTION_SKIP_TO_NEXT or PlaybackState.ACTION_SKIP_TO_PREVIOUS or
                        PlaybackState.ACTION_SEEK_TO,
                )
                .setState(state, pos, 1f)
                .build(),
        )
        session?.setMetadata(
            android.media.MediaMetadata.Builder()
                .putString(android.media.MediaMetadata.METADATA_KEY_TITLE, track.name)
                .putString(android.media.MediaMetadata.METADATA_KEY_ARTIST, track.artistNames)
                .putString(android.media.MediaMetadata.METADATA_KEY_ALBUM, track.albumName)
                .putLong(android.media.MediaMetadata.METADATA_KEY_DURATION, track.durationMs)
                .build(),
        )
    }

    private fun releasePlayer() {
        stopTicker()
        player?.release()
        player = null
    }

    override fun onDestroy() {
        stopTicker()
        releasePlayer()
        session?.release()
        session = null
        if (instance === this) instance = null
        super.onDestroy()
    }
}
