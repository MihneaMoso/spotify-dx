package com.spotifydx.app

import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.view.View
import android.widget.FrameLayout
import android.widget.ImageButton
import android.widget.ImageView
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

    /**
     * Last tag showCached was asked to display (verifyNav target). Updated
     * on every navigation so a superseded check never escalates.
     */
    private var expectedTag: String? = null

    /** Last emergencyHome escalation (bounded: one per 10s, no storms). */
    private var lastEmergencyMs = 0L

    // Mini-player view refs, bound once (renderPlayerBar runs on every
    // 250ms position tick — repeated findViewById would churn per tick).
    private var barTitle: TextView? = null
    private var barSubtitle: TextView? = null
    private var barArt: ImageView? = null
    private var barPlay: ImageButton? = null
    private var barScrub: SeekBar? = null
    private var barPos: TextView? = null
    private var barDuration: TextView? = null

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

        bindNav()
        bindPlayerBar()
        // Cache the per-tick mini-player refs once (see fields).
        findViewById<View>(R.id.player_bar)?.let { bar ->
            barTitle = bar.findViewById(R.id.player_title)
            barSubtitle = bar.findViewById(R.id.player_subtitle)
            barArt = bar.findViewById(R.id.mini_art)
            barPlay = bar.findViewById(R.id.btn_play)
            barScrub = bar.findViewById(R.id.scrub)
            barPos = bar.findViewById(R.id.player_pos)
            barDuration = bar.findViewById(R.id.player_duration)
        }
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
                // Same hasToken rule as the settled collector: a stored
                // (unverified) token stays shell-first; only tokenless
                // boots take the fast GATE lane.
                if ((settledInTime || !PlaybackStore.hasPersistedState()) &&
                    !SessionRepository.snapshot().hasToken
                ) {
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
                    !SessionRepository.snapshot().hasToken &&
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
        android.util.Log.d("SpotifyDxNav", "handleBack sheet=${playerSheet.isOpen} current=$current tab=$currentTab")
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
                if (stack.isEmpty()) {
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
        android.util.Log.d("SpotifyDxNav", "popTabToRoot tab=$tab size=${stack.size} current=$current")
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
        android.util.Log.d("SpotifyDxNav", "goBack tab=$currentTab size=${stack.size} current=$current")
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
        }
        // Allowing state loss: navigation is driven by async repo state
        // (session/watchdog collectors) that can legally emit after
        // onSaveInstanceState (backgrounded app) — a lost frame beats a
        // crash (IllegalStateException seen on-device 2026-09-09).
        android.util.Log.d("SpotifyDxNav", "showCached dest=$dest tag=$tag")
        expectedTag = tag
        tx.commitAllowingStateLoss()
        // Highlight the TAB (not the screen): a detail drilled from Search
        // keeps Search lit, matching the stack it belongs to.
        syncNav(currentTab)
        verifyNav(tag)
    }

    /**
     * Post-commit visibility check: the reinstall-only wedge class (back +
     * Home tab dead on a detail, no crash, reopen doesn't fix, reinstall
     * does — seen 2026-09-18 artist page, 2026-09-20 home playlist) leaves
     * the requested screen never-visible. If the expected tag isn't the
     * visible one once the transaction lands, escalate to [emergencyHome]
     * instead of stranding the user. Bounded (one escalation per 10s) and
     * supersede-safe (a newer navigation cancels an older check).
     */
    private fun verifyNav(tag: String) {
        findViewById<View>(R.id.content)?.post {
            // A newer navigation superseded this check — not a miss.
            if (expectedTag != tag) return@post
            val fm = supportFragmentManager
            runCatching { fm.executePendingTransactions() }
            val visible = fm.fragments.firstOrNull {
                it.isAdded && !it.isHidden && it.id == R.id.content
            }
            if (visible == null || visible.tag != tag) {
                android.util.Log.w(
                    "SpotifyDxNav",
                    "verifyNav MISS expected=$tag visible=${visible?.tag}",
                )
                val now = android.os.SystemClock.uptimeMillis()
                if (now - lastEmergencyMs > 10_000) {
                    lastEmergencyMs = now
                    emergencyHome()
                }
            } else {
                android.util.Log.d("SpotifyDxNav", "verifyNav OK $tag")
            }
        }
    }

    /**
     * Last-resort nav reset (see [verifyNav]): drops every back stack and
     * cached screen and rebuilds HOME fresh. Whatever poisoned the path —
     * desynced stacks, an orphaned fragment, a wedged transaction — a
     * clean rebuild routes around it with two back presses instead of a
     * reinstall. Login-scoped by construction (GATE path untouched).
     */
    private fun emergencyHome() {
        android.util.Log.w("SpotifyDxNav", "emergencyHome: resetting nav to HOME")
        clearScreens()
        tabStacks.clear()
        currentTab = Destination.HOME
        current = Destination.HOME
        lastArgs = null
        val tag = tagFor(Destination.HOME, null)
        stackFor(Destination.HOME).addLast(ScreenEntry(tag, Destination.HOME, null))
        // The 10s escalation bound above stops any verify→emergency loop:
        // this inner showCached re-arms expectedTag, and its verify can
        // only log on a second consecutive miss.
        showCached(Destination.HOME, null, tag)
    }

    private fun createFragment(dest: Destination, args: Bundle?): Fragment = when (dest) {
        Destination.GATE -> GateFragment()
        Destination.HOME -> HomeFragment()
        Destination.SEARCH -> SearchFragment()
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

    /** Settings entry point (the Home header badge — the top bar is gone). */
    fun goSettings() {
        go(Destination.SETTINGS)
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
        // The gate is chromeless: no nav (player-bar visibility
        // belongs to renderPlayerBar's track-driven ownership — syncNav
        // must not force it visible on every navigation).
        val gated = dest == Destination.GATE
        findViewById<BottomNavigationView>(R.id.bottom_nav)?.visibility =
            if (gated) View.GONE else View.VISIBLE
        findViewById<View>(R.id.nav_rail)?.visibility =
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
            // Gate on state AND settle: the mirror often doesn't change when
            // settle lands (empty stays empty, StateFlow conflates), so
            // observing state alone never re-evaluated (stranded-on-HOME).
            kotlinx.coroutines.flow.combine(
                SessionRepository.state,
                SessionRepository.settledFlow,
            ) { s, settled -> s to settled }.collect { (s, settled) ->
                if (s.authenticated) {
                    loginManager.hide()
                    if (current == Destination.GATE) go(Destination.HOME, null, true)
                } else {
                    // GATE only on definitively signed-OUT (settled AND no
                    // token at all): a present-but-unverified token stays on
                    // the shell while verification/capture proves it (the
                    // old conflated flow never re-fired here — that silence
                    // WAS the no-flash behavior; the settle flow exists so
                    // the genuinely empty case still routes).
                    if ((settled || bootGateArmed) && !s.hasToken &&
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
        var downY = 0f
        var downMs = 0L
        bar.setOnTouchListener { _, e ->
            fun tryOpen() {
                val now = android.os.SystemClock.uptimeMillis()
                if (now - lastOpenMs > 500) {
                    lastOpenMs = now
                    openPlayer()
                }
            }
            when (e.action) {
                android.view.MotionEvent.ACTION_DOWN -> {
                    lastY = e.y
                    accDy = 0f
                    downY = e.y
                    downMs = android.os.SystemClock.uptimeMillis()
                    true
                }
                android.view.MotionEvent.ACTION_MOVE -> {
                    val dy = lastY - e.y
                    lastY = e.y
                    if (dy > 0) {
                        accDy += dy
                        if (accDy > 150) {
                            accDy = 0f
                            tryOpen()
                        }
                    }
                    true
                }
                // Tap (not drag) on the bar background opens the sheet too
                // (Spotify parity). Controls/SeekBar consume their own
                // streams first, so this only sees background taps.
                android.view.MotionEvent.ACTION_UP -> {
                    val moved = kotlin.math.abs(e.y - downY)
                    val held = android.os.SystemClock.uptimeMillis() - downMs
                    if (moved < 24 && held < 500) tryOpen()
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
        // Compare-before-write throughout: this runs on every position tick,
        // so identical binds must not touch views (each write re-renders).
        val title = s.track?.name?.ifEmpty { "Not playing" } ?: "Not playing"
        if (barTitle?.text?.toString() != title) barTitle?.text = title
        val subtitle = s.track?.artistNames ?: ""
        if (barSubtitle?.text?.toString() != subtitle) barSubtitle?.text = subtitle
        // Mini artwork: tag-guarded so position ticks don't restart the
        // Coil request every emission; PLAYER rounding (12dp) matches the
        // pill's rounded language at thumbnail scale (the pill's own 28dp
        // would render a 48dp thumb fully circular).
        val art = barArt
        if (art != null) {
            val url = s.track?.coverUrl ?: ""
            if (art.getTag(R.id.mini_art) != url) {
                art.setTag(R.id.mini_art, url)
                ArtworkLoader.load(art, url, ArtworkLoader.Art.PLAYER)
            }
        }
        barPlay?.let {
            // Same-resource sets still invalidate; gate on last state.
            if (it.getTag(R.id.btn_play) != s.isPlaying) {
                it.setTag(R.id.btn_play, s.isPlaying)
                it.setImageResource(
                    if (s.isPlaying) android.R.drawable.ic_media_pause else android.R.drawable.ic_media_play,
                )
            }
        }
        // Transport is core-driven; until Phase 4 wires it the buttons show
        // state but issue no fake commands (PlayerRepository reverts).
        val scrub = barScrub
        if (scrub != null && s.durationMs > 0) {
            val max = s.durationMs.toInt()
            if (scrub.max != max) scrub.max = max
            if (!scrub.isPressed) scrub.progress = s.positionMs.toInt()
        }
        val pos = TrackAdapter.formatDuration(s.positionMs)
        if (barPos?.text?.toString() != pos) barPos?.text = pos
        val duration = TrackAdapter.formatDuration(s.durationMs)
        if (barDuration?.text?.toString() != duration) barDuration?.text = duration
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
