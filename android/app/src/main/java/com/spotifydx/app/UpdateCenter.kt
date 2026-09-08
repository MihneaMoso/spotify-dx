package com.spotifydx.app

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

/**
 * Updater state (§11 migration, §12 ARCHITECTURE): the checker/downloader/
 * stager stay native (hash verification, asset matching, atomic staging);
 * the existing `SpotifyDxUpdater` + file provider are first-class source
 * sets. Check-once-per-process plus manual check/apply wiring mirror today's
 * gates; the update prompt surfaces only on staged-ready.
 */
object UpdateCenter {
    data class State(
        val status: String = "No update check has run yet.",
        val updateAvailable: Boolean = false,
        val latest: String = "",
        val checking: Boolean = false,
        val downloading: Boolean = false,
        val stagedReady: Boolean = false,
    )

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val _state = MutableStateFlow(State())
    val state: StateFlow<State> = _state.asStateFlow()

    private var bootCheckDone = false

    fun checkAtBootIfEnabled() {
        if (bootCheckDone) return
        bootCheckDone = true
        if (!SettingsStore.settings.value.autoCheckUpdates) return
        check()
    }

    fun check() {
        if (_state.value.checking) return
        _state.value = _state.value.copy(checking = true)
        scope.launch {
            val res = BridgeClient.checkForUpdates()
            val json = res.getOrNull()
            if (json != null) {
                _state.value = _state.value.copy(
                    checking = false,
                    updateAvailable = json.optBoolean("update_available", false),
                    latest = json.optString("latest", ""),
                    status = if (json.optBoolean("update_available", false)) {
                        "Update available: v${json.optString("latest", "")}"
                    } else {
                        "Up to date (v${json.optString("current", "")})"
                    },
                )
            } else {
                _state.value = _state.value.copy(checking = false)
                refreshStatus()
                ToastBus.fromBridge(res.exceptionOrNull() ?: return@launch)
            }
        }
    }

    fun download() {
        if (_state.value.downloading) return
        _state.value = _state.value.copy(downloading = true)
        scope.launch {
            val res = BridgeClient.downloadUpdate()
            _state.value = _state.value.copy(downloading = false)
            if (res.isSuccess) {
                _state.value = _state.value.copy(stagedReady = true)
            } else {
                ToastBus.fromBridge(res.exceptionOrNull() ?: return@launch)
            }
            refreshStatus()
        }
    }

    fun apply(activity: android.app.Activity) {
        scope.launch {
            // Suspended on Dispatchers.IO inside BridgeClient; the installer
            // intent needs no main-thread affinity beyond the call itself.
            val res = BridgeClient.applyUpdate(activity)
            if (res.isFailure) ToastBus.fromBridge(res.exceptionOrNull() ?: return@launch)
            refreshStatus()
        }
    }

    fun refreshStatus() {
        scope.launch {
            val s = BridgeClient.updateStatus().getOrNull() ?: return@launch
            val cur = _state.value
            // `data` is a JSON string; strip the quotes for display.
            val text = s.trim('"')
            if (cur.status != text) _state.value = cur.copy(status = text)
        }
    }

    fun onEvent(e: EventBus.AppEvent) {
        if (e is EventBus.AppEvent.Update) refreshStatus()
    }
}
