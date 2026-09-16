package com.spotifydx.app

import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.view.View
import android.widget.FrameLayout
import android.widget.ImageButton
import android.widget.ImageView
import android.widget.SearchView
import android.widget.SeekBar
import android.widget.TextView
import androidx.appcompat.app.AppCompatActivity
import androidx.activity.addCallback
import androidx.core.app.ActivityCompat
import androidx.fragment.app.Fragment
import androidx.lifecycle.lifecycleScope
import com.google.android.material.bottomnavigation.BottomNavigationView
import com.google.android.material.navigationrail.NavigationRailView
import kotlinx.coroutines.Job
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

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
    private lateinit var playerSheet: PlayerSheetController
    /** Theme painted in onCreate (pre-store-load); see collectRepos. */
    private var appliedTheme: String? = null
    private var themeReconciled = false

    companion object {
        private const val KEY_DEST = "current_destination"
        private const val KEY_TAB = "current_tab"
        const val MAX_CACHED_SCREENS = 8
    }

    override fun onSaveInstanceState(outState: Bundle) {
        super.onSaveInstanceState(outState)
        outState.putString(KEY_DEST, current.name)
        outState.putString(KEY_TAB, currentTab.name)
    }
    private lateinit var toastView: TextView
    private var toastJob: Job? = null
    private var searchHandoff: String? = null

    private var current: Destination = Destination.GATE

    /** Last navigation args (copied — fragments must not alias these). */
    private var lastArgs: Bundle? = null

    enum class Destination { GATE, HOME, SEARCH, LIBRARY, SETTINGS, DETAIL }

    override fun onCreate(savedInstanceState: Bundle?) {
        // FragmentManager restores the visible fragment itself, but the
        // navigation fields reset — resync them or system back reads a
        // stale destination and exits from sub-screens. Per-tab stacks
        // restart empty (fragment instances are adopted on demand).
        if (savedInstanceState != null) {
            runCatching {
                currentTab = Destination.valueOf(
                    savedInstanceState.getString(KEY_TAB) ?: Destination.HOME.name,
                )
                current = Destination.valueOf(
                    savedInstanceState.getString(KEY_DEST) ?: Destination.GATE.name,
                )
            }
        }        // The store loads async (bridge); this paints from the in-memory
        // value, which is the default on cold start. collectRepos() recreates
        // once below if the loaded theme disagrees.
        appliedTheme = Theme.current()
        setTheme(Theme.themeRes(requireNotNull(appliedTheme)))
        super.onCreate(savedInstanceState)
        setContentView(R.layout.activity_main)

        loginOverlay = findViewById(R.id.login_overlay)
        toastView = findViewById(R.id.toast)
        playerSheet = PlayerSheetController(this).also {
            it.bind(findViewById(android.R.id.content))
        }
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
        // Boot auth: no fire-and-forget session flights here (concurrent
        // native session calls wedged the bridge and hung boot). The
        // settled-gate in collectRepos owns routing; refresh/verify run
        // from the application scope and the watchdog.
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
        // System back (gesture or button) mirrors the top-left back button:
        // sub-screens land on HOME instead of exiting the app. On HOME/GATE
        // finish() keeps the platform exit — WITHOUT disabling the callback:
        // a disabled callback stays dead for the activity instance, so one
        // back-press on HOME/GATE would silently break every later
        // sub-screen back (permanent-exit bug).
        // Single back behavior for gesture, button, and top-bar chevron:
        // sheet first, then back-stack pop, then root, then exit.
        onBackPressedDispatcher.addCallback(this) { handleBack() }

        if (savedInstanceState == null) {
            // Shell-first: HOME immediately, session resolves underneath.
            // The settled-gate above routes to GATE if and only if the core
            // definitively reports signed-out — no gate copy ever flashes
            // for valid sessions, no splash traps boot either.
            go(Destination.HOME)
            // Fast fresh-device path: a blank store proves no prior login,
            // so don't wait out a slow core — open the login flow as soon
            // as a short settle window lapses with no session. Stored state
            // (or a fast answer) keeps the flash-free settled path.
            lifecycleScope.launch {
                val settledInTime = SessionRepository.awaitSettled(3_000)
                if (SessionRepository.snapshot().authenticated) return@launch
                if (settledInTime || !PlaybackStore.hasPersistedState()) {
                    if (current != Destination.GATE) go(Destination.GATE, null, true)
                }
                // Else: probable valid session on a slow core — stay on
                // HOME; the settled collector and the backstop below finish
                // the job without any gate flash.
            }
            // Backstop for wedged devices: if the mirror never settles (core
            // not answering), an unauthenticated user must still reach the
            // login flow within seconds — stranding on a dead HOME with a
            // dead retry is the worse evil. Arm late enough that healthy
            // boots (settled in ~2s) never notice it.
            lifecycleScope.launch {
                kotlinx.coroutines.delay(15_000)
                bootGateArmed = true
                if (!SessionRepository.snapshot().authenticated &&
                    current != Destination.GATE
                ) {
                    go(Destination.GATE, null, true)
                }
            }
        }
    }

    /**
     * Single back behavior for gesture, system button, and top-bar
     * chevron: sheet first, then back-stack pop, then root, then exit.
     */
    private fun handleBack() {
        // Open player sheet: expanded section collapses first (Echo
        // parity), then the sheet minimizes.
        if (playerSheet.isOpen) {
            if (!playerSheet.backToMain()) playerSheet.close()
        } else if (goBack()) {
            // Back history popped (previous screen restored).
        } else if (current != Destination.HOME && current != Destination.GATE) {
            go(Destination.HOME)
        } else {
            finish()
        }
    }

    /** Full-screen player sheet (persistent overlay — see PlayerSheetController). */
    fun openPlayer() {
        playerSheet.open()
    }

    // -- Navigation ------------------------------------------------------------------
    /**
     * Shown-screen cache (state persistence across navigation): visited
     * screens are HIDDEN, never destroyed, keyed by destination (+kind/id
     * for details) — so ViewModels, loaded data, and view state survive
     * tab switches and back walks instead of reloading defaults. LRU-
     * capped; evicted screens rebuild fresh on revisit (scroll memory
     * still restores their viewport). Cleared on login transitions so no
     * other account's screens can resurface.
     */
    private val fragCache = LinkedHashMap<String, Fragment>()
    /**
     * Per-tab back stacks (Spotify model): every tab resumes exactly where
     * you left it — playlist included. Tapping a tab reveals its top;
     * drilling (openDetail) pushes onto the current tab; back pops within
     * the tab, then falls back to the HOME tab, then exits. GATE resets
     * everything (never leak screens across login). Entries carry tags so
     * pops return the SAME cached instance (show/hide cache) — never a
     * rebuild.
     */
    private data class ScreenEntry(val tag: String, val dest: Destination, val args: Bundle?)

    private val tabStacks = mutableMapOf<Destination, ArrayDeque<ScreenEntry>>()
    private var currentTab: Destination = Destination.HOME

    /** True while syncNav drives the highlight (listener must stay out). */
    private var syncingNav = false

    private fun stackFor(tab: Destination): ArrayDeque<ScreenEntry> =
        tabStacks.getOrPut(tab) { ArrayDeque() }

    private fun trimStack(stack: ArrayDeque<ScreenEntry>) {
        while (stack.size > 25) stack.removeFirst()
    }

    private fun tagFor(dest: Destination, args: Bundle?): String = when (dest) {
        Destination.DETAIL ->
            "DETAIL:${args?.getString("kind")}:${args?.getString("id")}"
        else -> dest.name
    }

    fun go(dest: Destination, args: Bundle? = null, clearStack: Boolean = false) {
        if (clearStack) {
            clearScreens()
            tabStacks.clear()
        }
        when (dest) {
            Destination.GATE -> {
                currentTab = Destination.HOME
                current = dest
                lastArgs = null
                showCached(dest, null)
            }
            Destination.HOME, Destination.SEARCH, Destination.LIBRARY, Destination.SETTINGS -> {
                currentTab = dest
                val stack = stackFor(dest)
                // Fresh top-bar query always opens a new search screen
                // (back returns to the previous one); otherwise resume.
                if (stack.isEmpty() || (dest == Destination.SEARCH && searchHandoff != null)) {
                    val tag = tagFor(dest, args)
                    stack.addLast(ScreenEntry(tag, dest, args?.let { Bundle(it) }))
                    trimStack(stack)
                    current = dest
                    lastArgs = args?.let { Bundle(it) }
                } else {
                    val top = stack.last()
                    current = top.dest
                    lastArgs = top.args?.let { Bundle(it) }
                }
                val top = stackFor(currentTab).last()
                showCached(top.dest, top.args?.let { Bundle(it) }, top.tag)
            }
            Destination.DETAIL -> {
                val tag = tagFor(dest, args)
                val stack = stackFor(currentTab)
                if (stack.lastOrNull()?.tag != tag) {
                    stack.addLast(ScreenEntry(tag, dest, args?.let { Bundle(it) }))
                    trimStack(stack)
                }
                current = dest
                lastArgs = args?.let { Bundle(it) }
                showCached(dest, lastArgs)
            }
        }
    }

    /** Active-tab reselect: pop to the tab root (Spotify behavior). */
    private fun popTabToRoot(tab: Destination) {
        val stack = stackFor(tab)
        while (stack.size > 1) stack.removeLast()
        val root = stack.lastOrNull()
        if (root == null) {
            val tag = tagFor(tab, null)
            stack.addLast(ScreenEntry(tag, tab, null))
            currentTab = tab
            current = tab
            lastArgs = null
            showCached(tab, null)
        } else {
            currentTab = tab
            current = root.dest
            lastArgs = root.args?.let { Bundle(it) }
            showCached(root.dest, lastArgs, root.tag)
        }
    }

    /** Pops back history (back gesture/button). False when already at root. */
    private fun goBack(): Boolean {
        val stack = stackFor(currentTab)
        if (stack.size > 1) {
            stack.removeLast()
            val top = stack.last()
            current = top.dest
            lastArgs = top.args?.let { Bundle(it) }
            showCached(top.dest, lastArgs, top.tag)
            return true
        }
        if (currentTab != Destination.HOME) {
            currentTab = Destination.HOME
            val home = stackFor(Destination.HOME)
            val top = home.lastOrNull()
            if (top == null) {
                val tag = tagFor(Destination.HOME, null)
                home.addLast(ScreenEntry(tag, Destination.HOME, null))
                current = Destination.HOME
                lastArgs = null
                showCached(Destination.HOME, null)
            } else {
                current = top.dest
                lastArgs = top.args?.let { Bundle(it) }
                showCached(top.dest, lastArgs, top.tag)
            }
            return true
        }
        return false
    }

    /** Drops every cached screen (login transitions only). */
    private fun clearScreens() {
        if (fragCache.isEmpty()) return
        val tx = supportFragmentManager.beginTransaction()
        fragCache.values.forEach { if (it.isAdded) tx.remove(it) }
        tx.commitAllowingStateLoss()
        fragCache.clear()
    }

    private fun showCached(dest: Destination, args: Bundle?, knownTag: String? = null) {
        val tag = knownTag ?: tagFor(dest, args)
        val fm = supportFragmentManager
        // Flush pending transactions first: hide decisions below read live
        // attachment state, and an un-executed add (cold start, theme
        // recreate) would otherwise be invisible to the hide loop while
        // still rendering on top afterwards.
        fm.executePendingTransactions()
        val tx = fm.beginTransaction()
        // Hide EVERYTHING attached in our container — not just the cache:
        // a process/activity-restored fragment the cache never saw (theme
        // recreate, death restore) stays visible forever otherwise, which
        // is exactly the first-switch overlay.
        fm.fragments.forEach {
            if (it.isAdded && it.id == R.id.content && it.tag != tag) tx.hide(it)
        }
        var frag: Fragment? = fragCache[tag] ?: fm.findFragmentByTag(tag)
        if (frag == null) {
            frag = createFragment(dest, args)
            tx.add(R.id.content, frag, tag)
            fragCache[tag] = frag
            // LRU evict the eldest hidden screen past the cap.
            while (fragCache.size > MAX_CACHED_SCREENS) {
                val eldest = fragCache.keys.firstOrNull { it != tag } ?: break
                fragCache.remove(eldest)?.let { if (it.isAdded) tx.remove(it) }
            }
        } else {
            // Touch for LRU + adopt process-restored instances.
            fragCache.remove(tag)
            fragCache[tag] = frag
            if (frag.isAdded) tx.show(frag)
            else tx.add(R.id.content, frag, tag)
            // Fresh top-bar query into a live Search screen.
            if (dest == Destination.SEARCH && frag is SearchFragment) {
                searchHandoff?.let {
                    searchHandoff = null
                    frag.submitExternal(it)
                }
            }
        }
        // Allowing state loss: navigation is driven by async repo state
        // (session/watchdog collectors) that can legally emit after
        // onSaveInstanceState (backgrounded app) — a lost frame beats a
        // crash (IllegalStateException seen on-device 2026-09-09).
        tx.commitAllowingStateLoss()
        // Highlight the TAB (not the screen): a detail drilled from Search
        // keeps Search lit, matching the stack it belongs to.
        syncNav(currentTab)
    }

    private fun createFragment(dest: Destination, args: Bundle?): Fragment = when (dest) {
        Destination.GATE -> GateFragment()
        Destination.HOME -> HomeFragment()
        Destination.SEARCH -> SearchFragment().apply {
            arguments = (args ?: Bundle()).apply {
                searchHandoff?.let { putString("handoff", it) }
            }
            searchHandoff = null
        }
        Destination.LIBRARY -> LibraryFragment()
        Destination.SETTINGS -> SettingsFragment()
        Destination.DETAIL -> DetailFragment().apply { arguments = args }
    }

    fun openDetail(kind: String, id: String, title: String) {
        go(Destination.DETAIL, Bundle().apply {
            putString("kind", kind)
            putString("id", id)
            putString("title", title)
        })
    }

    private fun syncNav(dest: Destination) {
        // Guarded writes: NavigationBarView.setOnItemSelectedListener fires
        // on programmatic setSelectedItemId too, so syncNav runs muted —
        // otherwise select -> go -> syncNav recurses until the stack blows
        // (the old id-compare guard broke the moment the visible screen
        // stopped matching the highlighted tab, i.e. every drill-down).
        val target = when (dest) {
            Destination.HOME -> R.id.nav_home
            Destination.SEARCH -> R.id.nav_search
            Destination.LIBRARY -> R.id.nav_library
            else -> null
        }
        if (target != null) {
            syncingNav = true
            try {
                findViewById<BottomNavigationView>(R.id.bottom_nav)?.let {
                    if (it.selectedItemId != target) it.selectedItemId = target
                }
                findViewById<NavigationRailView>(R.id.nav_rail)?.let {
                    if (it.selectedItemId != target) it.selectedItemId = target
                }
            } finally {
                syncingNav = false
            }
        }
        // The gate is chromeless: no top bar or nav (player-bar visibility
        // belongs to renderPlayerBar's track-driven ownership — syncNav
        // must not force it visible on every navigation).
        val gated = dest == Destination.GATE
        findViewById<View>(R.id.bottom_nav)?.visibility =
            if (gated) View.GONE else View.VISIBLE
        findViewById<View>(R.id.nav_rail)?.visibility =
            if (gated) View.GONE else View.VISIBLE
        findViewById<View>(R.id.top_bar)?.visibility =
            if (gated) View.GONE else View.VISIBLE
    }

    // -- Gate switch (single boolean, §4.2) --------------------------------------------
    // Shell-first (Echo parity: no gate flash): cold start lands on HOME
    // immediately and the auto-GATE below only fires once the session
    // mirror holds a DEFINITIVE core answer (see settled), or the boot
    // backstop below trips. Valid sessions therefore never flash gate
    // copy; transient bridge failures surface as page-local error/retry
    // instead of a gate detour. A fresh signed-out device still reaches
    // GATE in seconds (settled signed-out) — never stranded on a dead
    // HOME whose retry just re-logouts into the same error.
    private var bootGateArmed = false

    private fun collectRepos() {
        lifecycleScope.launch {
            SessionRepository.state.collect { s ->
                if (s.authenticated) {
                    loginManager.hide()
                    if (current == Destination.GATE) go(Destination.HOME, null, true)
                } else {
                    if ((SessionRepository.isSettled() || bootGateArmed) &&
                        current != Destination.GATE
                    ) {
                        go(Destination.GATE, null, true)
                    }
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
        // Chevron mirrors system back exactly (shared handler above).
        findViewById<View>(R.id.btn_back)?.setOnClickListener { handleBack() }
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
        findViewById<View>(R.id.avatar_photo)?.let { Design.clipCircle(it) }
        refreshProfileBadge()
    }

    override fun onResume() {
        super.onResume()
        // Profile may have changed in Settings (name save, avatar set or
        // cleared) — re-read on every return.
        refreshProfileBadge()
    }

    /** Top-right profile badge: username + circular avatar photo. */
    private fun refreshProfileBadge() {
        val name = findViewById<TextView>(R.id.avatar_name) ?: return
        val photo = findViewById<ImageView>(R.id.avatar_photo) ?: return
        lifecycleScope.launch {
            val json = withContext(Dispatchers.IO) {
                BridgeClient.getProfile().getOrNull()
            }
            name.text = json?.optString("username", "").orEmpty()
            val b64 = json?.optString("avatar_b64", "").orEmpty()
            if (b64.isEmpty()) {
                photo.setImageResource(android.R.drawable.ic_menu_myplaces)
                return@launch
            }
            val bmp = withContext(Dispatchers.IO) { decodeAvatar(b64) }
            if (bmp != null) photo.setImageBitmap(bmp)
            else photo.setImageResource(android.R.drawable.ic_menu_myplaces)
        }
    }

    /** Downsamples avatar bytes to badge size (cheap, no cache needed). */
    private fun decodeAvatar(b64: String): android.graphics.Bitmap? = runCatching {
        val bytes = android.util.Base64.decode(b64, android.util.Base64.DEFAULT)
        if (bytes.isEmpty()) return null
        val bounds = android.graphics.BitmapFactory.Options().apply { inJustDecodeBounds = true }
        android.graphics.BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
        var sample = 1
        while (bounds.outWidth / (sample * 2) >= 96 && bounds.outHeight / (sample * 2) >= 96) {
            sample *= 2
        }
        val opts = android.graphics.BitmapFactory.Options().apply { inSampleSize = sample }
        android.graphics.BitmapFactory.decodeByteArray(bytes, 0, bytes.size, opts)
    }.getOrNull()

    // -- Bottom nav / rail (same breakpoints as the responsive contract) ------------------
    private fun bindNav() {
        val select: (Int) -> Boolean = { id ->
            if (syncingNav) {
                // Programmatic highlight sync — never a navigation.
                true
            } else {
                val dest = when (id) {
                    R.id.nav_home -> Destination.HOME
                    R.id.nav_search -> Destination.SEARCH
                    R.id.nav_library -> Destination.LIBRARY
                    else -> null
                }
                // Switching tabs resumes each tab's top (playlist included);
                // tapping the ACTIVE tab pops it to root (Spotify behavior).
                if (dest == null) {
                    true
                } else if (dest == currentTab) {
                    popTabToRoot(dest)
                    true
                } else {
                    go(dest)
                    true
                }
            }
        }
        findViewById<BottomNavigationView>(R.id.bottom_nav)?.setOnItemSelectedListener { item ->
            select(item.itemId)
        }
        findViewById<NavigationRailView>(R.id.nav_rail)?.setOnItemSelectedListener { item ->
            select(item.itemId)
        }
    }

    // -- Player bar (Echo floating mini-player: transport + titles + scrub) -------------------------
    private fun bindPlayerBar() {
        val bar = findViewById<View>(R.id.player_bar) ?: return
        // Swipe up opens the full-screen player sheet (Spotify parity).
        // Taps still hit the bar's own controls (buttons/SeekBars consume
        // their own streams first — this listener only sees background
        // touches). Tracked MANUALLY, not via GestureDetector: the detector
        // needs its own DOWN bookkeeping and silently drops streams whose
        // DOWN it never saw, which made opens flaky. A 150px upward run —
        // slow drag or fast fling alike — opens exactly once (500ms debounce
        // covers the async fragment-commit window).
        // TEMP-DIAG gestures.
        var lastY = 0f
        var accDy = 0f
        var lastOpenMs = 0L
        bar.setOnTouchListener { _, e ->
            when (e.action) {
                android.view.MotionEvent.ACTION_DOWN -> {
                    lastY = e.y
                    accDy = 0f
                    true
                }
                android.view.MotionEvent.ACTION_MOVE -> {
                    val dy = lastY - e.y
                    lastY = e.y
                    if (dy > 0) {
                        accDy += dy
                        if (accDy > 150) {
                            accDy = 0f
                            val now = android.os.SystemClock.uptimeMillis()
                            if (now - lastOpenMs > 500) {
                                lastOpenMs = now
                                openPlayer()
                            }
                        }
                    }
                    true
                }
                else -> false
            }
        }
        bar.findViewById<ImageButton>(R.id.btn_play)?.setOnClickListener {
            PlayerRepository.toggle()
        }
        bar.findViewById<ImageButton>(R.id.btn_next)?.setOnClickListener {
            PlayerRepository.nextTrack()
        }
        bar.findViewById<ImageButton>(R.id.btn_prev)?.setOnClickListener {
            PlayerRepository.seekTo(0)
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
    }

    private fun renderPlayerBar(s: PlayerRepository.State) {
        val bar = findViewById<View>(R.id.player_bar) ?: return
        // Single owner for bar visibility: it shows if and only if a track
        // exists (restored or playing). The old gated-only toggling left it
        // hidden after cold starts that never passed a second syncNav.
        bar.visibility = if (s.track != null) View.VISIBLE else View.GONE
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
            TrackAdapter.formatDuration(s.positionMs)
        bar.findViewById<TextView>(R.id.player_duration)?.text =
            TrackAdapter.formatDuration(s.durationMs)
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
}
