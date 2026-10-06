package com.spotifydx.app

import android.app.Application
import android.util.Log
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch

/**
 * The native runtime is owned by the Application, not the Activity (§12.5):
 * rotation, multi-window, and process recreation never restart downloads,
 * decoding, or the session. View state is the only thing allowed to reset.
 */
class SpotifyDxApp : Application() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)

    override fun onCreate() {
        super.onCreate()
        // Process-wide singletons must not depend on any Activity: a
        // START_STICKY-restarted service can run before MainActivity:init.
        AppState.init(this)
        AudioCache.init(this)
        // Warm the WebView cookie singleton on the interface thread NOW:
        // its first touch initializes the backing provider (UI-thread only)
        // and every session path (fast refresh, revive, login) assumes it
        // exists. An off-thread first touch aborts the process.
        runCatching { android.webkit.CookieManager.getInstance() }
        scope.launch(Dispatchers.IO) {
            try {
                BridgeClient.checkVersion()
            } catch (e: BridgeException) {
                Log.e(TAG, "Refusing to proceed: ${e.error}")
                // Unlatch too: a latched-never gate turns every screen
                // into a 20s readiness hang instead of the immediate
                // protocol error this refusal already logged.
                BridgeClient.markReady()
                return@launch
            }
            val filesDir = filesDir.absolutePath
            val cacheDir = cacheDir.absolutePath
            val outcome = BridgeClient.initCore(filesDir, cacheDir)
            // Unlatch the bridge gate either way: success proceeds, failure
            // surfaces real errors per call instead of hanging at readiness.
            BridgeClient.markReady()
            if (outcome.isFailure) {
                Log.e(TAG, "initCore failed: ${outcome.exceptionOrNull()}")
                return@launch
            }
            Log.i(TAG, "core ready: ${outcome.getOrNull()}")
            EventBus.start()
            SessionRepository.refresh()
        }
        scope.launch(Dispatchers.Main) {
            EventBus.events.collect { e ->
                SessionRepository.onEvent(e)
                UpdateCenter.onEvent(e)
            }
        }
    }

    companion object {
        const val TAG = "SpotifyDx"
    }
}
