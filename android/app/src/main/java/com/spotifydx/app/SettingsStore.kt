package com.spotifydx.app

import androidx.appcompat.app.AppCompatDelegate
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import org.json.JSONObject

/**
 * Settings + profile stores (§11 migration, §13 ARCHITECTURE): same
 * documents, same defaults-on-failure, same validation, same
 * snapshot-then-save discipline as the current build — reached over the
 * bridge instead of direct file access.
 */
object SettingsStore {
    data class Settings(
        val theme: String = "deep-blue",
        val volume: Float = 0.8f,
        val engine: String = "auto",
        val hideUpsell: Boolean = false,
        val autoCheckUpdates: Boolean = true,
        /** Phase F credential tier: opaque tokens, local-only, never logged. */
        val qobuzAppId: String = "",
        val qobuzAuthToken: String = "",
        val tidalToken: String = "",
        val deezerArl: String = "",
    )

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val _settings = MutableStateFlow(Settings())
    val settings: StateFlow<Settings> = _settings.asStateFlow()

    fun load() {
        scope.launch {
            val json = BridgeClient.getSettings().getOrNull() ?: return@launch
            applyJson(json)
        }
    }

    fun save(next: Settings) {
        val prev = _settings.value
        _settings.value = next
        Theme.apply(next.theme)
        scope.launch(Dispatchers.IO) {
            val json = JSONObject()
                .put("theme", kebab(next.theme))
                .put("volume", next.volume.toDouble())
                .put("engine", next.engine)
                .put("hide_upsell", next.hideUpsell)
                .put("auto_check_updates", next.autoCheckUpdates)
                .put("qobuz_app_id", next.qobuzAppId)
                .put("qobuz_auth_token", next.qobuzAuthToken)
                .put("tidal_token", next.tidalToken)
                .put("deezer_arl", next.deezerArl)
                .toString()
            BridgeClient.setSettings(json).onFailure {
                _settings.value = prev
                ToastBus.error("Could not save settings")
            }
        }
    }

    private fun applyJson(json: JSONObject) {
        val next = Settings(
            theme = json.optString("theme", "deep-blue"),
            volume = json.optDouble("volume", 0.8).toFloat().coerceIn(0f, 1f),
            engine = json.optString("engine", "auto"),
            hideUpsell = json.optBoolean("hide_upsell", false),
            autoCheckUpdates = json.optBoolean("auto_check_updates", true),
            qobuzAppId = json.optString("qobuz_app_id", ""),
            qobuzAuthToken = json.optString("qobuz_auth_token", ""),
            tidalToken = json.optString("tidal_token", ""),
            deezerArl = json.optString("deezer_arl", ""),
        )
        if (next != _settings.value) _settings.value = next
        Theme.apply(next.theme)
    }

    private fun kebab(theme: String): String = when (theme) {
        "onyx" -> "onyx"
        else -> "deep-blue"
    }
}

/**
 * Theme application (§7.4 migration): port of the design-token set 1:1
 * (surfaces, text, accent, radii — see `colors.xml`, mirrored from
 * `src/ui/theme.rs`). Theme switch is an instant repaint with no data
 * reload; both themes are dark (the app has no light theme).
 */
object Theme {
    const val DEEP_BLUE = "deep-blue"
    const val ONYX = "onyx"

    fun apply(theme: String) {
        // Both palettes are dark; night mode stays forced so WebViews and
        // dialogs never flash light. The overlay choice is read by activities
        // via [themeRes] before setContentView.
        AppCompatDelegate.setDefaultNightMode(AppCompatDelegate.MODE_NIGHT_YES)
    }

    fun themeRes(theme: String): Int = when (theme) {
        ONYX -> R.style.Theme_SpotifyDx_Onyx
        else -> R.style.Theme_SpotifyDx
    }

    fun current(): String = SettingsStore.settings.value.theme
}
