package com.spotifydx.app

import android.app.Activity
import android.content.Intent
import android.os.Bundle
import android.view.LayoutInflater
import android.view.View
import android.view.ViewGroup
import android.widget.Button
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.RadioButton
import android.widget.RadioGroup
import android.widget.TextView
import androidx.fragment.app.Fragment
import androidx.lifecycle.lifecycleScope
import kotlinx.coroutines.launch

/**
 * Settings screen (§13 ARCHITECTURE, §11 migration): profile (name + avatar),
 * appearance, playback-engine choice, privacy stats, updates, cache note.
 * Local stores + updater state only — no music data. Every control here is
 * live in Phase 1: settings/profile round-trip over the bridge, filter stats
 * from the engine, update check/download/install end to end.
 */
class SettingsFragment : Fragment() {
    override fun onCreateView(i: LayoutInflater, c: ViewGroup?, s: Bundle?): View =
        i.inflate(R.layout.fragment_settings, c, false)

    override fun onViewCreated(v: View, s: Bundle?) {
        bindProfile(v)
        bindAppearance(v)
        bindEngine(v)
        bindQuality(v)
        bindCredentials(v)
        bindPrivacy(v)
        bindUpdates(v)
        bindCacheNote(v)
    }

    // -- Profile: display name + avatar ---------------------------------------------------
    private fun bindProfile(v: View) {
        val name: EditText = v.findViewById(R.id.profile_name)
        val save: Button = v.findViewById(R.id.profile_save)
        val avatar: Button = v.findViewById(R.id.profile_avatar)
        val clear: Button = v.findViewById(R.id.profile_avatar_clear)
        val status: TextView = v.findViewById(R.id.profile_status)

        viewLifecycleOwner.lifecycleScope.launch {
            val json = BridgeClient.getProfile().getOrNull()
            // Defaults-on-failure: a missing profile never blocks the screen.
            name.setText(json?.optString("username", "") ?: "")
            val hasAvatar = !(json?.optString("avatar_b64", "") ?: "").isNullOrEmpty()
            status.text = if (hasAvatar) "Avatar set" else "No avatar"
        }
        save.setOnClickListener {
            viewLifecycleOwner.lifecycleScope.launch {
                val res = BridgeClient.setProfileName(name.text.toString())
                status.text = if (res.isSuccess) "Saved" else "Save failed"
                res.exceptionOrNull()?.let { ToastBus.fromBridge(it) }
            }
        }
        avatar.setOnClickListener {
            @Suppress("DEPRECATION")
            startActivityForResult(
                Intent(Intent.ACTION_GET_CONTENT).setType("image/*"),
                REQ_AVATAR,
            )
        }
        clear.setOnClickListener {
            viewLifecycleOwner.lifecycleScope.launch {
                BridgeClient.clearAvatar()
                status.text = "Avatar removed"
            }
        }
        v.findViewById<Button>(R.id.profile_logout)?.setOnClickListener {
            (activity as? MainActivity)?.logout()
        }
    }

    @Deprecated("Legacy picker hook; replaced by the Photo Picker in hardening.")
    @Suppress("DEPRECATION")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        if (requestCode != REQ_AVATAR || resultCode != Activity.RESULT_OK) return
        val uri = data?.data ?: return
        viewLifecycleOwner.lifecycleScope.launch {
            // Avatar bytes cross JNI once (§6.4); core validates signature +
            // size cap and reports a typed error otherwise.
            val bytes = requireContext().contentResolver.openInputStream(uri)?.use { it.readBytes() }
                ?: return@launch
            val mime = requireContext().contentResolver.getType(uri) ?: "image/png"
            val res = BridgeClient.setAvatar(bytes, mime)
            view?.findViewById<TextView>(R.id.profile_status)?.text =
                if (res.isSuccess) "Avatar saved" else "Avatar rejected"
            res.exceptionOrNull()?.let { ToastBus.fromBridge(it) }
        }
    }

    // -- Appearance --------------------------------------------------------------------------
    private fun bindAppearance(v: View) {
        val group: RadioGroup = v.findViewById(R.id.theme_group)
        val deep: RadioButton = v.findViewById(R.id.theme_deep)
        val onyx: RadioButton = v.findViewById(R.id.theme_onyx)
        viewLifecycleOwner.lifecycleScope.launch {
            SettingsStore.settings.collect { s ->
                // Null-listener bind: programmatic check() must never fire
                // the listener, or opening Settings with a non-default theme
                // spuriously saves + recreates the activity.
                group.setOnCheckedChangeListener(null)
                group.check(if (s.theme == Theme.ONYX) R.id.theme_onyx else R.id.theme_deep)
                group.setOnCheckedChangeListener { _, checked ->
                    val cur = SettingsStore.settings.value
                    val next = cur.copy(theme = if (checked == R.id.theme_onyx) Theme.ONYX else Theme.DEEP_BLUE)
                    if (next.theme == cur.theme) return@setOnCheckedChangeListener
                    SettingsStore.save(next)
                    // Theme switch is an instant repaint with no data reload.
                    activity?.recreate()
                }
            }
        }
        // Keep references live for the (otherwise unused) lookup above.
        deep.isEnabled = true
        onyx.isEnabled = true
    }

    // -- Playback engine radios -------------------------------------------------------------------
    private fun bindEngine(v: View) {
        val group: RadioGroup = v.findViewById(R.id.engine_group)
        viewLifecycleOwner.lifecycleScope.launch {
            SettingsStore.settings.collect { s ->
                group.setOnCheckedChangeListener(null)
                group.check(
                    when (s.engine) {
                        "spotify-sdk" -> R.id.engine_sdk
                        "open" -> R.id.engine_open
                        else -> R.id.engine_auto
                    },
                )
                // Snapshot-then-save: a crash cannot persist a half-written choice.
                group.setOnCheckedChangeListener { _, checked ->
                    val cur = SettingsStore.settings.value
                    val engine = when (checked) {
                        R.id.engine_sdk -> "spotify-sdk"
                        R.id.engine_open -> "open"
                        else -> "auto"
                    }
                    if (engine == cur.engine) return@setOnCheckedChangeListener
                    SettingsStore.save(cur.copy(engine = engine))
                }
            }
        }
        v.findViewById<View>(R.id.upsell_toggle)?.setOnClickListener {
            val cur = SettingsStore.settings.value
            SettingsStore.save(cur.copy(hideUpsell = !cur.hideUpsell))
        }
    }

    // -- Streaming quality selectors (wifi cap + metered cap) -------------------------------
    private fun bindQuality(v: View) {
        val wifiGroup: LinearLayout = v.findViewById(R.id.quality_wifi_group)
        val mobileGroup: LinearLayout = v.findViewById(R.id.quality_mobile_group)
        // Rows built once; the settings collect below only flips checks
        // (compare-before-write, so no ripple churn on unrelated saves).
        val wifiRows = buildQualityRows(wifiGroup, get = { it.qualityWifi }) { q ->
            val cur = SettingsStore.settings.value
            if (q != cur.qualityWifi) SettingsStore.save(cur.copy(qualityWifi = q))
        }
        val mobileRows = buildQualityRows(mobileGroup, get = { it.qualityMobile }) { q ->
            val cur = SettingsStore.settings.value
            if (q != cur.qualityMobile) SettingsStore.save(cur.copy(qualityMobile = q))
        }
        viewLifecycleOwner.lifecycleScope.launch {
            SettingsStore.settings.collect { s ->
                wifiRows.forEach { (q, btn) ->
                    btn.isChecked = q == s.qualityWifi
                }
                mobileRows.forEach { (q, btn) ->
                    btn.isChecked = q == s.qualityMobile
                }
            }
        }
    }

    /** Builds one tappable option row per tier; returns (quality, radio) pairs. */
    private fun buildQualityRows(
        group: LinearLayout,
        get: (SettingsStore.Settings) -> String,
        onPick: (String) -> Unit,
    ): List<Pair<String, RadioButton>> {
        val ctx = group.context
        val names = mapOf(
            SettingsStore.Quality.LOW to ctx.getString(R.string.quality_low),
            SettingsStore.Quality.NORMAL to ctx.getString(R.string.quality_normal),
            SettingsStore.Quality.HIGH to ctx.getString(R.string.quality_high),
            SettingsStore.Quality.LOSSLESS to ctx.getString(R.string.quality_lossless),
        )
        val cur = SettingsStore.settings.value
        return SettingsStore.Quality.ALL.map { q ->
            val row = LayoutInflater.from(ctx).inflate(R.layout.item_quality_option, group, false)
            val btn: RadioButton = row.findViewById(R.id.quality_option_btn)
            row.findViewById<TextView>(R.id.quality_option_name).text = names[q]
            row.findViewById<TextView>(R.id.quality_option_detail).text =
                SettingsStore.Quality.detail(q)
            btn.isChecked = q == get(cur)
            row.setOnClickListener { onPick(q) }
            group.addView(row)
            q to btn
        }
    }

    // -- Streaming credentials (Phase F tier: opaque, local-only) ----------------
    private fun bindCredentials(v: View) {
        val appId: EditText = v.findViewById(R.id.cred_qobuz_app)
        val token: EditText = v.findViewById(R.id.cred_qobuz_token)
        val tidal: EditText = v.findViewById(R.id.cred_tidal)
        val deezer: EditText = v.findViewById(R.id.cred_deezer)
        val status: TextView = v.findViewById(R.id.cred_status)
        fun fill(s: SettingsStore.Settings) {
            // Don't clobber in-progress typing on unrelated emissions.
            if (!appId.isFocused) appId.setText(s.qobuzAppId)
            if (!token.isFocused) token.setText(s.qobuzAuthToken)
            if (!tidal.isFocused) tidal.setText(s.tidalToken)
            if (!deezer.isFocused) deezer.setText(s.deezerArl)
            status.text = if (s.qobuzAppId.isNotEmpty() && s.qobuzAuthToken.isNotEmpty()) {
                "Qobuz active"
            } else {
                ""
            }
        }
        viewLifecycleOwner.lifecycleScope.launch {
            SettingsStore.settings.collect { fill(it) }
        }
        v.findViewById<Button>(R.id.cred_save)?.setOnClickListener {
            val cur = SettingsStore.settings.value
            SettingsStore.save(
                cur.copy(
                    qobuzAppId = appId.text.toString().trim(),
                    qobuzAuthToken = token.text.toString().trim(),
                    tidalToken = tidal.text.toString().trim(),
                    deezerArl = deezer.text.toString().trim(),
                ),
            )
            status.text = "Saved"
        }
    }

    // -- Privacy panel (same counters as the current build) -------------------------------------------
    private fun bindPrivacy(v: View) {
        val stats: TextView = v.findViewById(R.id.privacy_stats)
        val refresh: Button = v.findViewById(R.id.privacy_refresh)
        fun load() {
            viewLifecycleOwner.lifecycleScope.launch {
                val json = BridgeClient.filterStats().getOrNull()
                stats.text = if (json != null) {
                    "Rules: ${json.optInt("tracked", 0)} · " +
                        "Blocked: ${json.optInt("blocked", 0)} · " +
                        "List failures: ${json.optLong("ad_fetch_failures", 0)}"
                } else {
                    "Stats unavailable"
                }
            }
        }
        refresh.setOnClickListener {
            viewLifecycleOwner.lifecycleScope.launch {
                BridgeClient.refreshFilters()
                load()
            }
        }
        load()
    }

    // -- Updates (prompt surfaces only on staged-ready) ---------------------------------------------------
    private fun bindUpdates(v: View) {
        val status: TextView = v.findViewById(R.id.update_status)
        val check: Button = v.findViewById(R.id.update_check)
        val download: Button = v.findViewById(R.id.update_download)
        val apply: Button = v.findViewById(R.id.update_apply)
        viewLifecycleOwner.lifecycleScope.launch {
            UpdateCenter.state.collect { s ->
                status.text = s.status
                check.isEnabled = !s.checking
                download.isEnabled = s.updateAvailable && !s.downloading
                // The install prompt exists only once a payload is staged.
                apply.visibility = if (s.stagedReady) View.VISIBLE else View.GONE
            }
        }
        check.setOnClickListener { UpdateCenter.check() }
        download.setOnClickListener { UpdateCenter.download() }
        apply.setOnClickListener { UpdateCenter.apply(requireActivity()) }
    }

    private fun bindCacheNote(v: View) {
        v.findViewById<TextView>(R.id.cache_note)?.text =
            "Artwork and stream caches live in the app's private storage and are pruned automatically."
        val size: TextView? = v.findViewById(R.id.cache_size)
        val clear: Button? = v.findViewById(R.id.cache_clear)
        fun refreshSize() {
            viewLifecycleOwner.lifecycleScope.launch {
                val (bytes, pinned) = AudioCache.sizeInfo()
                val mb = bytes / (1024 * 1024)
                val capMb = AudioCache.MAX_BYTES / (1024 * 1024)
                size?.text = "Music cache: $mb MB / $capMb MB" +
                    if (pinned > 0) " ($pinned pinned)" else ""
            }
        }
        refreshSize()
        clear?.setOnClickListener {
            clear.isEnabled = false
            viewLifecycleOwner.lifecycleScope.launch {
                AudioCache.clear()
                refreshSize()
                clear.isEnabled = true
            }
        }
    }

    companion object {
        private const val REQ_AVATAR = 71
    }
}
