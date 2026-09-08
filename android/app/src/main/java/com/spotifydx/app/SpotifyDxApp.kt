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
        scope.launch(Dispatchers.IO) {
            try {
                BridgeClient.checkVersion()
            } catch (e: BridgeException) {
                Log.e(TAG, "Refusing to proceed: ${e.error}")
                return@launch
            }
            val filesDir = filesDir.absolutePath
            val cacheDir = cacheDir.absolutePath
            val outcome = BridgeClient.initCore(filesDir, cacheDir)
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
