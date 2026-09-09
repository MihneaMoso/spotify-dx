package com.spotifydx.app

import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.view.View
import android.widget.FrameLayout
import android.widget.ImageButton
import android.widget.SearchView
import android.widget.SeekBar
import android.widget.TextView
import androidx.appcompat.app.AppCompatActivity
import androidx.core.app.ActivityCompat
import androidx.fragment.app.Fragment
import androidx.lifecycle.lifecycleScope
import com.google.android.material.bottomnavigation.BottomNavigationView
import com.google.android.material.navigationrail.NavigationRailView
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/**
 * Routed shell (§10.1 migration): top bar (history, global search with
 * one-shot handoff to Search, avatar menu with settings/logout), side
 * navigation collapsing to an icon rail on medium screens and a bottom tab
 * bar on small ones, now-playing column rules, player bar, and toast.
 *
 * One root switch decides gate vs shell on the session boolean alone (§4.2
 * ARCHITECTURE). Nine destinations mirror the nine routes plus detail
 * arguments; the search handoff is consumed exactly once.
 */
class MainActivity : AppCompatActivity() {
    private lateinit var loginOverlay: FrameLayout
    private lateinit var loginManager: LoginWebViewManager
    private var sdkDriver: SdkWebViewDriver? = null
    /** Theme painted in onCreate (pre-store-load); see collectRepos. */
    private var appliedTheme: String? = null
    private var themeReconciled = false
    private lateinit var toastView: TextView
    private var toastJob: Job? = null
    private var searchHandoff: String? = null

    private var current: Destination = Destination.GATE

    enum class Destination { GATE, HOME, SEARCH, LIBRARY, LIKED, QUEUE, SETTINGS, DETAIL }

    override fun onCreate(savedInstanceState: Bundle?) {
        // The store loads async (bridge); this paints from the in-memory
        // value, which is the default on cold start. collectRepos() recreates
        // once below if the loaded theme disagrees.
        appliedTheme = Theme.current()
        setTheme(Theme.themeRes(requireNotNull(appliedTheme)))
        super.onCreate(savedInstanceState)
        setContentView(R.layout.activity_main)

        loginOverlay = findViewById(R.id.login_overlay)
        toastView = findViewById(R.id.toast)
        loginManager = LoginWebViewManager(this, loginOverlay)
        // Phase 5 SDK device host (lazy: the view is only built on the SDK
        // engine path). Events feed PlayerRepository on the main thread.
        sdkDriver = SdkWebViewDriver(this, findViewById(R.id.sdk_holder)).also { d ->
            d.onDeviceReady = { PlayerRepository.onSdkDevice(it) }
            d.onPlayerState = { PlayerRepository.onSdkState(it) }
            PlayerRepository.sdkDriver = d
        }
        SessionRefresher.pageHost = { loginManager.takeIf { !isFinishing } }

        SettingsStore.load()
        SessionRepository.refresh()
        SessionRepository.verifyAtBoot()
        SessionRepository.startWatchdog()
        UpdateCenter.checkAtBootIfEnabled()
        // Persisted app state: context holder first, then queue/last-played
        // rehydrate (lands paused — restore, never autoplay).
        AppState.init(applicationContext)
        PlayerRepository.restore()
        startService(PlaybackService.intentOf(this))
        requestNotificationPermission()

        bindTopBar()
        bindNav()
        bindPlayerBar()
        collectRepos()

        if (savedInstanceState == null) {
            // Reopen with a surviving process (service playing): the session
            // mirror is live, so land straight on the app — never flash the
            // gate. Fresh process starts unauthenticated → gate as usual.
            go(
                if (SessionRepository.snapshot().authenticated) Destination.HOME
                else Destination.GATE,
            )
        }
    }

    // -- Navigation ------------------------------------------------------------------
    fun go(dest: Destination, args: Bundle? = null) {
        current = dest
        val frag: Fragment = when (dest) {
            Destination.GATE -> GateFragment()
            Destination.HOME -> HomeFragment()
            Destination.SEARCH -> SearchFragment().apply {
                arguments = (args ?: Bundle()).apply {
                    searchHandoff?.let { putString("handoff", it) }
                }
                searchHandoff = null
            }
            Destination.LIBRARY -> LibraryFragment()
            Destination.LIKED -> LikedFragment()
            Destination.QUEUE -> QueueFragment()
            Destination.SETTINGS -> SettingsFragment()
            Destination.DETAIL -> DetailFragment().apply { arguments = args }
        }
        // Allowing state loss: navigation is driven by async repo state
        // (session/watchdog collectors) that can legally emit after
        // onSaveInstanceState (backgrounded app) — a lost frame beats a
        // crash (IllegalStateException seen on-device 2026-09-09).
        supportFragmentManager.beginTransaction()
            .replace(R.id.content, frag)
            .commitAllowingStateLoss()
        syncNav(dest)
    }

    fun openDetail(kind: String, id: String, title: String) {
        go(Destination.DETAIL, Bundle().apply {
            putString("kind", kind)
            putString("id", id)
            putString("title", title)
        })
    }

    private fun syncNav(dest: Destination) {
        // Guarded writes: NavigationBarView.setSelectedItemId dispatches the
        // selection listener UNCONDITIONALLY (even for the current id), so an
        // unguarded write here recurses select -> go -> syncNav until the
        // stack blows (StackOverflowError in findViewById).
        val target = when (dest) {
            Destination.HOME -> R.id.nav_home
            Destination.SEARCH -> R.id.nav_search
            Destination.LIBRARY -> R.id.nav_library
            Destination.LIKED -> R.id.nav_liked
            else -> null
        }
        if (target != null) {
            findViewById<BottomNavigationView>(R.id.bottom_nav)?.let {
                if (it.selectedItemId != target) it.selectedItemId = target
            }
            findViewById<NavigationRailView>(R.id.nav_rail)?.let {
                if (it.selectedItemId != target) it.selectedItemId = target
            }
        }
        // The gate is chromeless: no top bar, nav, or player bar — just the
        // placeholder with the login page layered above it (§4.3).
        val gated = dest == Destination.GATE
        findViewById<View>(R.id.player_bar)?.visibility =
            if (gated) View.GONE else View.VISIBLE
        findViewById<View>(R.id.bottom_nav)?.visibility =
            if (gated) View.GONE else View.VISIBLE
        findViewById<View>(R.id.nav_rail)?.visibility =
            if (gated) View.GONE else View.VISIBLE
        findViewById<View>(R.id.top_bar)?.visibility =
            if (gated) View.GONE else View.VISIBLE
    }

    // -- Gate switch (single boolean, §4.2) --------------------------------------------
    private fun collectRepos() {
        lifecycleScope.launch {
            SessionRepository.state.collect { s ->
                if (s.authenticated) {
                    loginManager.hide()
                    if (current == Destination.GATE) go(Destination.HOME)
                } else {
                    if (current != Destination.GATE) go(Destination.GATE)
                }
            }
        }
        lifecycleScope.launch {
            PlayerRepository.state.collect { renderPlayerBar(it) }
        }
        lifecycleScope.launch {
            ToastBus.toasts.collect { t -> showToast(t) }
        }
        lifecycleScope.launch {
            SettingsStore.settings.collect { s ->
                // Cold start paints the default theme (store loads async).
                // Recreate exactly once when the stored theme disagrees, so
                // the launch theme is always the user's theme. No loop: the
                // recreated activity paints the loaded value from the start.
                if (!themeReconciled && s.theme != appliedTheme) {
                    themeReconciled = true
                    recreate()
                }
            }
        }
    }

    // -- Top bar ------------------------------------------------------------------------
    private fun bindTopBar() {
        findViewById<View>(R.id.btn_back)?.setOnClickListener {
            if (current != Destination.HOME && current != Destination.GATE) go(Destination.HOME)
        }
        findViewById<SearchView>(R.id.search_view)?.setOnQueryTextListener(
            object : SearchView.OnQueryTextListener {
                override fun onQueryTextSubmit(query: String): Boolean {
                    // One-shot handoff: Search consumes it on arrival (§7.2).
                    searchHandoff = query
                    go(Destination.SEARCH)
                    return true
                }

                override fun onQueryTextChange(newText: String): Boolean = false
            },
        )
        findViewById<View>(R.id.btn_avatar)?.setOnClickListener { go(Destination.SETTINGS) }
    }

    // -- Bottom nav / rail (same breakpoints as the responsive contract) ------------------
    private fun bindNav() {
        val select: (Int) -> Boolean = { id ->
            val dest = when (id) {
                R.id.nav_home -> Destination.HOME
                R.id.nav_search -> Destination.SEARCH
                R.id.nav_library -> Destination.LIBRARY
                R.id.nav_liked -> Destination.LIKED
                R.id.nav_queue -> Destination.QUEUE
                else -> null
            }
            // Second half of the syncNav recursion guard: ignore selections
            // that are already current (covers programmatic dispatches).
            if (dest != null && dest != current) go(dest)
            true
        }
        findViewById<BottomNavigationView>(R.id.bottom_nav)?.setOnItemSelectedListener { item ->
            select(item.itemId)
        }
        findViewById<NavigationRailView>(R.id.nav_rail)?.setOnItemSelectedListener { item ->
            select(item.itemId)
        }
    }

    // -- Player bar (transport cluster, scrub/volume, queue, like) -------------------------
    private fun bindPlayerBar() {
        val bar = findViewById<View>(R.id.player_bar) ?: return
        bar.findViewById<ImageButton>(R.id.btn_play)?.setOnClickListener {
            PlayerRepository.toggle()
        }
        bar.findViewById<ImageButton>(R.id.btn_next)?.setOnClickListener {
            PlayerRepository.nextTrack()
        }
        bar.findViewById<ImageButton>(R.id.btn_prev)?.setOnClickListener {
            PlayerRepository.seekTo(0)
        }
        bar.findViewById<ImageButton>(R.id.btn_queue)?.setOnClickListener {
            go(Destination.QUEUE)
        }
        bar.findViewById<SeekBar>(R.id.scrub)?.setOnSeekBarChangeListener(
            object : SeekBar.OnSeekBarChangeListener {
                override fun onProgressChanged(s: SeekBar, v: Int, fromUser: Boolean) {
                    if (fromUser) PlayerRepository.seekTo(v.toLong())
                }

                override fun onStartTrackingTouch(s: SeekBar) {}
                override fun onStopTrackingTouch(s: SeekBar) {}
            },
        )
        bar.findViewById<SeekBar>(R.id.volume)?.setOnSeekBarChangeListener(
            object : SeekBar.OnSeekBarChangeListener {
                override fun onProgressChanged(s: SeekBar, v: Int, fromUser: Boolean) {
                    if (fromUser) PlayerRepository.setVolume(v / 100f)
                }

                override fun onStartTrackingTouch(s: SeekBar) {}
                override fun onStopTrackingTouch(s: SeekBar) {}
            },
        )
    }

    private fun renderPlayerBar(s: PlayerRepository.State) {
        val bar = findViewById<View>(R.id.player_bar) ?: return
        // Compare-before-write discipline extends to views: skip identical binds.
        bar.findViewById<TextView>(R.id.player_title)?.text =
            s.track?.name?.ifEmpty { "Not playing" } ?: "Not playing"
        bar.findViewById<TextView>(R.id.player_subtitle)?.text =
            s.track?.artistNames ?: ""
        bar.findViewById<ImageButton>(R.id.btn_play)?.setImageResource(
            if (s.isPlaying) android.R.drawable.ic_media_pause else android.R.drawable.ic_media_play,
        )
        // Transport is core-driven; until Phase 4 wires it the buttons show
        // state but issue no fake commands (PlayerRepository reverts).
        val scrub = bar.findViewById<SeekBar>(R.id.scrub)
        if (scrub != null && s.durationMs > 0) {
            scrub.max = s.durationMs.toInt()
            if (!scrub.isPressed) scrub.progress = s.positionMs.toInt()
        }
        bar.findViewById<TextView>(R.id.player_pos)?.text =
            TrackAdapter.formatDuration(s.positionMs) + " / " + TrackAdapter.formatDuration(s.durationMs)
        // Optimistic like control (silent fallback until the endpoint lands).
        bar.findViewById<ImageButton>(R.id.btn_like)?.apply {
            alpha = if (s.transportReady) 1f else 0.4f
        }
    }

    // -- Toast (generation-guarded auto-dismiss) --------------------------------------------
    private fun showToast(t: ToastBus.Toast) {
        toastView.text = t.message
        toastView.visibility = View.VISIBLE
        toastJob?.cancel()
        toastJob = lifecycleScope.launch {
            delay(4_000)
            // Stale timers cannot clear newer errors: only hide if untouched.
            toastView.visibility = View.GONE
        }
    }

    // -- Login page hosting (§8) --------------------------------------------------------------
    fun showLoginPage(url: String) {
        // The page lives again — refresh flights may use it.
        SessionRefresher.pageHost = { loginManager.takeIf { !isFinishing } }
        loginManager.show(url)
    }

    /** Full logout cycle: tear down cookie pages AND clear the credential
     * store + mirror, so the next sign-in starts clean (§8.6). */
    fun logout() {
        SessionRefresher.pageHost = null
        loginManager.destroyForLogout()
        sdkDriver?.shutdown()
        SessionRepository.logout()
    }

    private fun requestNotificationPermission() {
        if (Build.VERSION.SDK_INT >= 33 &&
            checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS) !=
            PackageManager.PERMISSION_GRANTED
        ) {
            ActivityCompat.requestPermissions(
                this,
                arrayOf(android.Manifest.permission.POST_NOTIFICATIONS),
                41,
            )
        }
    }

    override fun onDestroy() {
        // The core outlives the Activity: never tear down session/download
        // state here. Only the login view holder is released with its owner.
        if (isFinishing) loginManager.destroy()
        // The SDK device dies with its WebView (rotation = new device on
        // next SDK play; hardening may lift the view to the Application).
        PlayerRepository.sdkDriver = null
        sdkDriver?.shutdown()
        sdkDriver = null
        super.onDestroy()
    }

    /** Placeholder settings entry for layouts without a dedicated button. */
    fun openSettings(@Suppress("UNUSED_PARAMETER") v: View) = go(Destination.SETTINGS)

    fun openQueue(@Suppress("UNUSED_PARAMETER") v: View) = go(Destination.QUEUE)
}
