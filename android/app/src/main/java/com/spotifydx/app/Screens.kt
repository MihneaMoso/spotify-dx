package com.spotifydx.app

import android.os.Bundle
import android.view.LayoutInflater
import android.view.View
import android.view.ViewGroup
import android.widget.Button
import android.widget.EditText
import android.widget.ImageButton
import android.widget.ImageView
import android.widget.ProgressBar
import android.widget.TextView
import androidx.fragment.app.Fragment
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.lifecycleScope
import androidx.recyclerview.widget.LinearLayoutManager
import androidx.recyclerview.widget.RecyclerView
import kotlinx.coroutines.launch

/** Binds the shared (content/message/retry) state triple of a screen layout. */
private fun Fragment.bindState(
    root: View,
    state: ScreenState,
    contentId: Int = R.id.screen_content,
    messageId: Int = R.id.screen_message,
    retryId: Int = R.id.screen_retry,
) {
    UiStates.bind(
        state,
        root.findViewById(contentId),
        root.findViewById(messageId),
        root.findViewById(retryId),
    )
}

// -- Scroll memory -----------------------------------------------------------------------
/**
 * Per-screen scroll positions (Echo keeps list state across navigation;
 * our replace-based nav destroys views, so positions persist here
 * instead). Keyed per list ("home:shelf"), detail pages namespaced by
 * kind+id. Data itself already survives in repositories/caches — only the
 * viewport needs remembering.
 */
object ScrollMemory {
    private val pos = mutableMapOf<String, Pair<Int, Int>>()

    fun save(key: String, lm: LinearLayoutManager) {
        val p = lm.findFirstVisibleItemPosition()
        if (p == RecyclerView.NO_POSITION) return
        // Start-edge offset in the layout direction (top for vertical,
        // left for the horizontal shelf) — what scrollToPositionWithOffset
        // consumes to reproduce the exact viewport.
        val edge = lm.findViewByPosition(p)?.let {
            if (lm.orientation == LinearLayoutManager.HORIZONTAL) lm.getDecoratedLeft(it)
            else lm.getDecoratedTop(it)
        } ?: 0
        pos[key] = p to edge
    }

    fun restore(key: String, rv: RecyclerView) {
        val lm = rv.layoutManager as? LinearLayoutManager ?: return
        val (p, off) = pos[key] ?: return
        val count = rv.adapter?.itemCount ?: 0
        if (count == 0) return
        lm.scrollToPositionWithOffset(p.coerceIn(0, count - 1), off)
    }
}

/** Persist scroll on every move; restore once on first data delivery. */
fun RecyclerView.rememberScroll(key: String) {
    val lm = layoutManager as? LinearLayoutManager ?: return
    // Initial layout passes fire onScrolled at position 0 — saving those
    // would clobber the remembered position before the restore below runs.
    // Only saves count once the initial restore has landed.
    var restored = false
    addOnScrollListener(object : RecyclerView.OnScrollListener() {
        override fun onScrolled(rv: RecyclerView, dx: Int, dy: Int) {
            if (restored) ScrollMemory.save(key, lm)
        }
    })
    adapter?.registerAdapterDataObserver(object : RecyclerView.AdapterDataObserver() {
        var done = false
        override fun onChanged() = restoreOnce()
        override fun onItemRangeInserted(positionStart: Int, itemCount: Int) = restoreOnce()
        private fun restoreOnce() {
            if (done) return
            // Empty dispatches (the StateFlow's initial [] emission) must
            // NOT consume the one-shot: real data lands in a later dispatch.
            if ((adapter?.itemCount ?: 0) == 0) return
            done = true
            // Synchronous (data is committed): scrollToPosition pends safely
            // pre-layout, so no intermediate top-layout can fire clobbering
            // saves.
            ScrollMemory.restore(key, this@rememberScroll)
            restored = true
            adapter?.unregisterAdapterDataObserver(this)
        }
    })
}

// -- Gate --------------------------------------------------------------------------
/** Login gate (§4.3 ARCHITECTURE): branded placeholder while the sign-in flow
 * runs exactly once; failures surface an error with a retry control. */
class GateFragment : Fragment() {
    private val vm: LoginViewModel by lazy {
        ViewModelProvider(this)[LoginViewModel::class.java]
    }

    override fun onCreateView(i: LayoutInflater, c: ViewGroup?, s: Bundle?): View =
        i.inflate(R.layout.fragment_gate, c, false)

    override fun onViewCreated(v: View, s: Bundle?) {
        val status: TextView = v.findViewById(R.id.gate_status)
        val retry: Button = v.findViewById(R.id.gate_retry)
        val progress: ProgressBar = v.findViewById(R.id.gate_progress)
        retry.setOnClickListener {
            vm.reset()
            vm.begin()
        }
        viewLifecycleOwner.lifecycleScope.launch {
            vm.starting.collect { starting ->
                progress.visibility = if (starting) View.VISIBLE else View.GONE
                retry.visibility = if (starting) View.GONE else View.VISIBLE
                status.text = if (starting) "Opening login…" else "Sign in with Spotify to continue."
            }
        }
        // The core hands over the sign-in URL; showing the page consumes it
        // exactly once (re-entry guard lives in the ViewModel).
        viewLifecycleOwner.lifecycleScope.launch {
            vm.loginUrl.collect { url ->
                if (url != null) {
                    (activity as? MainActivity)?.showLoginPage(url)
                    vm.consumedUrl()
                }
            }
        }
        // Kick off the sign-in flow exactly once.
        if (s == null) {
            vm.begin()
        }
    }
}

// -- Home ----------------------------------------------------------------------------
class HomeFragment : Fragment() {
    private val vm: HomeViewModel by lazy {
        ViewModelProvider(this)[HomeViewModel::class.java]
    }

    override fun onCreateView(i: LayoutInflater, c: ViewGroup?, s: Bundle?): View =
        i.inflate(R.layout.fragment_home, c, false)

    override fun onViewCreated(v: View, s: Bundle?) {
        val greeting: TextView = v.findViewById(R.id.home_greeting)
        greeting.text = vm.greeting
        val banner: TextView = v.findViewById(R.id.home_banner)
        val list: RecyclerView = v.findViewById(R.id.home_list)
        // Playlist shelf scrolls horizontally (desktop .shelf-row parity:
        // fixed-width cards, square art on top). Liked tracks below stay a
        // vertical list, like desktop's track-list under the shelves.
        list.layoutManager =
            LinearLayoutManager(context, LinearLayoutManager.HORIZONTAL, false)
        val shelves = TitleAdapter(
            onClick = { pos ->
                vm.playlists.value.getOrNull(pos)?.let {
                    (activity as? MainActivity)?.openDetail("playlist", it.id, it.name)
                }
            },
            itemLayout = R.layout.item_card,
            onMenu = { pos ->
                vm.playlists.value.getOrNull(pos)?.let {
                    ContextMenuHost.showMenu(parentFragmentManager, MenuTarget.Playlist(it))
                }
            },
        )
        list.adapter = shelves
        list.rememberScroll("home:shelf")
        val likedList: RecyclerView = v.findViewById(R.id.home_liked)
        likedList.layoutManager = LinearLayoutManager(context)
        val liked = TrackAdapter(
            showIndex = false,
            onPlay = { PlayerRepository.play(it, "Liked Songs") },
            onMenu = { ContextMenuHost.showMenu(parentFragmentManager, MenuTarget.Song(it)) },
        )
        likedList.adapter = liked
        likedList.rememberScroll("home:liked")
        likedList.swipeToQueue(liked)
        viewLifecycleOwner.lifecycleScope.launch {
            vm.state.collect { bindState(v, it) }
        }
        viewLifecycleOwner.lifecycleScope.launch {
            vm.banner.collect { b ->
                banner.visibility = if (b == null) View.GONE else View.VISIBLE
                banner.text = b ?: ""
            }
        }
        viewLifecycleOwner.lifecycleScope.launch {
            vm.playlists.collect { ps ->
                shelves.submitList(ps.map {
                    val count = if (it.trackCount > 0) " · ${it.trackCount} tracks" else ""
                    TitleAdapter.Row(it.name, "Playlist$count", it.coverUrl)
                })
            }
        }
        viewLifecycleOwner.lifecycleScope.launch {
            vm.liked.collect { liked.submitList(it) }
        }
        // Cached screens re-attach with ViewModel (data) intact: load once
        // per instance, never per view. Rotation/rotation-death creates a
        // new instance (loaded=false) and loads correctly.
        if (!loaded) {
            loaded = true
            vm.load()
        }
    }

    /** View state is rebuilt; instance state (ViewModel) survives hides. */
    private var loaded = false
}

// -- Search ----------------------------------------------------------------------------
class SearchFragment : Fragment() {
    private val vm: SearchViewModel by lazy {
        ViewModelProvider(this)[SearchViewModel::class.java]
    }

    /** Forwards a fresh top-bar query into this live (cached) screen. */
    fun submitExternal(query: String) {
        view?.findViewById<EditText>(R.id.search_box)?.setText(query)
        vm.submit(query)
    }

    override fun onCreateView(i: LayoutInflater, c: ViewGroup?, s: Bundle?): View =
        i.inflate(R.layout.fragment_search, c, false)

    override fun onViewCreated(v: View, s: Bundle?) {
        val box: EditText = v.findViewById(R.id.search_box)
        val clear: ImageButton = v.findViewById(R.id.search_clear)
        // Unified results (songs → artists → albums, API order kept): one
        // list, track rows playable + swipeable, title rows drill down.
        val list: RecyclerView = v.findViewById(R.id.search_results)
        list.layoutManager = LinearLayoutManager(context)
        val adapter = SearchAdapter(
            onPlayTrack = { PlayerRepository.play(it, "Search") },
            onOpenAlbum = { id, name ->
                (activity as? MainActivity)?.openDetail("album", id, name)
            },
            onOpenArtist = { id, name ->
                (activity as? MainActivity)?.openDetail("artist", id, name)
            },
            onMenu = { ContextMenuHost.showMenu(parentFragmentManager, it) },
        )
        list.adapter = adapter
        list.rememberScroll("search:results")
        list.swipeToQueue(adapter)
        box.setOnEditorActionListener { tv, _, _ ->
            vm.submit(tv.text.toString())
            true
        }
        // Recent-search chips: visible only when the box is empty, history
        // is non-empty, and no results are showing. Tapping re-runs the query.
        val historyRow: View = v.findViewById(R.id.history_row)
        val history: com.google.android.material.chip.ChipGroup =
            v.findViewById(R.id.search_history)
        var recentCache: List<String> = emptyList()
        fun refreshHistory() {
            val show = box.text.isNullOrEmpty() && recentCache.isNotEmpty() &&
                vm.tracks.value.isEmpty() && vm.albums.value.isEmpty() &&
                vm.artists.value.isEmpty()
            historyRow.visibility = if (show) View.VISIBLE else View.GONE
            history.visibility = if (show) View.VISIBLE else View.GONE
            if (!show) return
            history.removeAllViews()
            recentCache.forEach { q ->
                history.addView(
                    com.google.android.material.chip.Chip(
                        android.view.ContextThemeWrapper(
                            context,
                            R.style.EchoFilterChip,
                        ),
                    ).apply {
                        text = q
                        setOnClickListener {
                            box.setText(q)
                            box.setSelection(q.length)
                            vm.submit(q)
                        }
                    },
                )
            }
        }
        v.findViewById<Button>(R.id.history_clear)?.setOnClickListener {
            vm.clearHistory()
        }
        box.addTextChangedListener(object : android.text.TextWatcher {
            override fun beforeTextChanged(s: CharSequence?, a: Int, b: Int, c: Int) {}
            override fun onTextChanged(s: CharSequence?, a: Int, b: Int, c: Int) {}
            override fun afterTextChanged(s: android.text.Editable?) {
                // X mirrors the top bar's close affordance: present exactly
                // while there is text to clear.
                clear.visibility = if (s.isNullOrEmpty()) View.GONE else View.VISIBLE
                if (s.isNullOrEmpty()) {
                    // Empty box (backspaced or X-cleared) drops stale
                    // results and returns to recent searches — the blank
                    // path clears synchronously, no fetch involved.
                    vm.submit("")
                }
                refreshHistory()
            }
        })
        // Programmatic clear routes through the watcher above (visibility,
        // result reset, history) — never duplicated here.
        clear.setOnClickListener { box.setText("") }
        // View recreation can restore box text without firing the watcher.
        clear.visibility = if (box.text.isNullOrEmpty()) View.GONE else View.VISIBLE
        viewLifecycleOwner.lifecycleScope.launch {
            vm.recent.collect {
                recentCache = it
                refreshHistory()
            }
        }
        viewLifecycleOwner.lifecycleScope.launch {
            vm.state.collect {
                bindState(v, it)
                refreshHistory()
            }
        }
        viewLifecycleOwner.lifecycleScope.launch {
            vm.results.collect { adapter.submitList(it) }
        }
        // One-shot handoff consumed on arrival, never re-seeded: drop it
        // from arguments so re-attaching this cached screen cannot replay
        // a stale query over live results.
        if (s == null) {
            vm.consumeHandoff(arguments?.getString("handoff"))
            arguments?.remove("handoff")
        }
    }
}

// -- Library ----------------------------------------------------------------------------
class LibraryFragment : Fragment() {
    private val vm: LibraryViewModel by lazy {
        ViewModelProvider(this)[LibraryViewModel::class.java]
    }

    override fun onCreateView(i: LayoutInflater, c: ViewGroup?, s: Bundle?): View =
        i.inflate(R.layout.fragment_library, c, false)

    override fun onViewCreated(v: View, s: Bundle?) {
        val filter: EditText = v.findViewById(R.id.library_filter)
        filter.setOnEditorActionListener { tv, _, _ ->
            vm.setFilter(tv.text.toString())
            true
        }
        val list: RecyclerView = v.findViewById(R.id.library_list)
        list.layoutManager = LinearLayoutManager(context)
        val rows = TitleAdapter(onClick = { pos ->
            val act = activity as? MainActivity ?: return@TitleAdapter
            when (val r = vm.rows.value.getOrNull(pos)) {
                is LibraryViewModel.LibraryRow.P ->
                    act.openDetail("playlist", r.p.id, r.p.name)
                is LibraryViewModel.LibraryRow.A ->
                    act.openDetail("album", r.a.id, r.a.name)
                is LibraryViewModel.LibraryRow.T -> PlayerRepository.play(r.t, "Liked Songs")
                null -> {}
            }
        }, onMenu = { pos ->
            val target = when (val r = vm.rows.value.getOrNull(pos)) {
                is LibraryViewModel.LibraryRow.P -> MenuTarget.Playlist(r.p)
                is LibraryViewModel.LibraryRow.A -> MenuTarget.Album(r.a)
                is LibraryViewModel.LibraryRow.T -> MenuTarget.Song(r.t)
                null -> null
            }
            target?.let { ContextMenuHost.showMenu(parentFragmentManager, it) }
        })
        list.adapter = rows
        list.rememberScroll("library:list")
        // Echo split-button look (tab_bg/tab_text selectors react to
        // selected; PLAYLISTS starts selected in XML to match the default).
        val libTabIds = listOf(R.id.tab_playlists, R.id.tab_albums, R.id.tab_liked)
        fun markLibTab(id: Int) {
            libTabIds.forEach { v.findViewById<Button>(it)?.isSelected = it == id }
        }
        // One-shot sync with the live tab (visual only — no new collector).
        markLibTab(
            when (vm.tab.value) {
                LibraryViewModel.Tab.ALBUMS -> R.id.tab_albums
                LibraryViewModel.Tab.LIKED -> R.id.tab_liked
                else -> R.id.tab_playlists
            },
        )
        v.findViewById<Button>(R.id.tab_playlists)?.setOnClickListener {
            markLibTab(R.id.tab_playlists)
            vm.selectTab(LibraryViewModel.Tab.PLAYLISTS)
        }
        v.findViewById<Button>(R.id.tab_albums)?.setOnClickListener {
            markLibTab(R.id.tab_albums)
            vm.selectTab(LibraryViewModel.Tab.ALBUMS)
        }
        v.findViewById<Button>(R.id.tab_liked)?.setOnClickListener {
            markLibTab(R.id.tab_liked)
            vm.selectTab(LibraryViewModel.Tab.LIKED)
        }
        viewLifecycleOwner.lifecycleScope.launch {
            vm.state.collect { bindState(v, it) }
        }
        viewLifecycleOwner.lifecycleScope.launch {
            vm.rows.collect { items ->
                rows.submitList(items.map { r ->
                    when (r) {
                        is LibraryViewModel.LibraryRow.P -> TitleAdapter.Row(
                            r.p.name,
                            if (r.p.trackCount > 0) "Playlist · ${r.p.trackCount} tracks" else "Playlist",
                            r.p.coverUrl,
                        )
                        is LibraryViewModel.LibraryRow.A -> TitleAdapter.Row(
                            r.a.name, r.a.artists.joinToString(", "), r.a.coverUrl,
                        )
                        is LibraryViewModel.LibraryRow.T -> TitleAdapter.Row(
                            r.t.name, r.t.artistNames, r.t.coverUrl,
                        )
                    }
                })
            }
        }
        // Same load-once rule as Home: cached re-attaches reuse the live
        // ViewModel (tab, filter, rows) instead of refetching defaults.
        if (!loaded) {
            loaded = true
            vm.load()
        }
    }

    private var loaded = false
}

// -- Album / Artist / Playlist detail (shared layout) -------------------------------------------
class DetailFragment : Fragment() {
    private val vm: DetailViewModel by lazy {
        ViewModelProvider(this)[DetailViewModel::class.java]
    }

    override fun onCreateView(i: LayoutInflater, c: ViewGroup?, s: Bundle?): View =
        i.inflate(R.layout.fragment_detail, c, false)

    override fun onViewCreated(v: View, s: Bundle?) {
        val title: TextView = v.findViewById(R.id.detail_title)
        val subtitle: TextView = v.findViewById(R.id.detail_subtitle)
        val art: ImageView = v.findViewById(R.id.detail_art)
        // Artist headers are circular (Echo avatar treatment); albums and
        // playlists keep the rounded player-art clip.
        val isArtist = (arguments?.getString("kind", "playlist") ?: "playlist") == "artist"
        val list: RecyclerView = v.findViewById(R.id.detail_list)
        list.layoutManager = LinearLayoutManager(context)
        val adapter = TrackAdapter(
            onPlay = { PlayerRepository.play(it, title.text.toString()) },
            onMenu = { ContextMenuHost.showMenu(parentFragmentManager, MenuTarget.Song(it)) },
        )
        list.adapter = adapter
        // Detail scroll namespaces by kind+id: each playlist/album/artist
        // remembers its own viewport independently.
        val detailKey =
            "detail:${arguments?.getString("kind", "playlist")}:${arguments?.getString("id", "")}"
        list.rememberScroll(detailKey)
        list.swipeToQueue(adapter)
        v.findViewById<Button>(R.id.detail_play)?.setOnClickListener {
            adapter.currentList.firstOrNull()
                ?.let { PlayerRepository.play(it, title.text.toString()) }
        }
        // Playlist sort (Spotify parity): popup menu anchored to the sort
        // button; the button label always shows the active order. Albums and
        // artists keep their natural order (button hidden).
        val detailKind = arguments?.getString("kind", "playlist") ?: "playlist"
        val sortBtn = v.findViewById<Button>(R.id.detail_sort)
        sortBtn?.visibility = if (detailKind == "playlist") View.VISIBLE else View.GONE
        // A fresh sort always lands on top (Echo/Spotify parity): without
        // this DiffUtil preserves the old scroll offset and the user ends
        // up at the bottom (or a random middle) of the new order.
        var sortJustChanged = false
        sortBtn?.setOnClickListener { anchor ->
            val menu = android.widget.PopupMenu(context, anchor)
            DetailViewModel.SortOrder.entries.forEachIndexed { i, order ->
                menu.menu.add(0, i, i, order.label).isCheckable = true
            }
            menu.menu.setGroupCheckable(0, true, true)
            menu.menu.getItem(vm.sort.value.ordinal)?.isChecked = true
            menu.setOnMenuItemClickListener { item ->
                DetailViewModel.SortOrder.entries.getOrNull(item.itemId)
                    ?.let {
                        if (it != vm.sort.value) {
                            sortJustChanged = true
                            vm.setSort(it)
                        }
                    }
                true
            }
            menu.show()
        }
        fun sortLabel(): Int = when (vm.sort.value) {
            DetailViewModel.SortOrder.TITLE -> R.string.sort_title
            DetailViewModel.SortOrder.ARTIST -> R.string.sort_artist
            DetailViewModel.SortOrder.ALBUM -> R.string.sort_album
            DetailViewModel.SortOrder.RECENT -> R.string.sort_recent
            DetailViewModel.SortOrder.CUSTOM -> R.string.sort_custom
        }
        viewLifecycleOwner.lifecycleScope.launch {
            vm.sort.collect { sortBtn?.setText(sortLabel()) }
        }
        viewLifecycleOwner.lifecycleScope.launch {
            vm.state.collect { bindState(v, it) }
        }
        viewLifecycleOwner.lifecycleScope.launch {
            vm.title.collect { title.text = it }
        }
        viewLifecycleOwner.lifecycleScope.launch {
            vm.subtitle.collect { subtitle.text = it }
        }
        viewLifecycleOwner.lifecycleScope.launch {
            vm.cover.collect { url ->
                ArtworkLoader.load(art, url, ArtworkLoader.Art.PLAYER)
                if (isArtist) Design.clipCircle(art)
            }
        }
        viewLifecycleOwner.lifecycleScope.launch {
            vm.tracks.collect {
                if (sortJustChanged) {
                    sortJustChanged = false
                    // Commit callback runs AFTER the async diff lands the
                    // new order: scrolling here can't be undone by a later
                    // dispatch (the previous post() raced it and lost).
                    adapter.submitList(it) { list.scrollToPosition(0) }
                } else {
                    adapter.submitList(it)
                }
            }
        }
        // Detail pages are keyed per kind+id in the screen cache: a cached
        // revisit reuses tracks AND position instead of reloading. Title
        // rebinds every attach (cheap, always correct).
        title.text = arguments?.getString("title", "") ?: ""
        if (!loaded) {
            loaded = true
            val kind = arguments?.getString("kind", "playlist") ?: "playlist"
            val id = arguments?.getString("id", "") ?: ""
            vm.load(kind, id)
        }
    }

    private var loaded = false
}
