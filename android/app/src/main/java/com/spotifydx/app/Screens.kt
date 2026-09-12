package com.spotifydx.app

import android.os.Bundle
import android.view.LayoutInflater
import android.view.View
import android.view.ViewGroup
import android.widget.Button
import android.widget.EditText
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
        )
        list.adapter = shelves
        val likedList: RecyclerView = v.findViewById(R.id.home_liked)
        likedList.layoutManager = LinearLayoutManager(context)
        val liked = TrackAdapter(showIndex = false, onPlay = { PlayerRepository.play(it, "Liked Songs") })
        likedList.adapter = liked
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
        if (s == null) vm.load()
    }
}

// -- Search ----------------------------------------------------------------------------
class SearchFragment : Fragment() {
    private val vm: SearchViewModel by lazy {
        ViewModelProvider(this)[SearchViewModel::class.java]
    }

    override fun onCreateView(i: LayoutInflater, c: ViewGroup?, s: Bundle?): View =
        i.inflate(R.layout.fragment_search, c, false)

    override fun onViewCreated(v: View, s: Bundle?) {
        val box: EditText = v.findViewById(R.id.search_box)
        val list: RecyclerView = v.findViewById(R.id.search_list)
        list.layoutManager = LinearLayoutManager(context)
        val adapter = TrackAdapter(showIndex = false, onPlay = { PlayerRepository.play(it, "Search") })
        list.adapter = adapter
        list.swipeToQueue(adapter)
        val albumList: RecyclerView = v.findViewById(R.id.search_albums)
        albumList.layoutManager = LinearLayoutManager(context)
        val albums = TitleAdapter(onClick = { pos ->
            vm.albums.value.getOrNull(pos)?.let {
                (activity as? MainActivity)?.openDetail("album", it.id, it.name)
            }
        })
        albumList.adapter = albums
        val artistList: RecyclerView = v.findViewById(R.id.search_artists)
        artistList.layoutManager = LinearLayoutManager(context)
        val artists = TitleAdapter(onClick = { pos ->
            vm.artists.value.getOrNull(pos)?.let {
                (activity as? MainActivity)?.openDetail("artist", it.id, it.name)
            }
        })
        artistList.adapter = artists
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
            override fun afterTextChanged(s: android.text.Editable?) = refreshHistory()
        })
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
            vm.tracks.collect { adapter.submitList(it) }
        }
        viewLifecycleOwner.lifecycleScope.launch {
            vm.albums.collect { items ->
                albums.submitList(items.map {
                    TitleAdapter.Row(it.name, it.artists.joinToString(", "), it.coverUrl)
                })
            }
        }
        viewLifecycleOwner.lifecycleScope.launch {
            vm.artists.collect { items ->
                artists.submitList(items.map { TitleAdapter.Row(it.name, "Artist", it.imageUrl) })
            }
        }
        // One-shot handoff consumed on arrival, never re-seeded.
        if (s == null) {
            vm.consumeHandoff(arguments?.getString("handoff"))
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
        })
        list.adapter = rows
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
        if (s == null) vm.load()
    }
}

// -- Liked (paged) -------------------------------------------------------------------------
class LikedFragment : Fragment() {
    private val vm: LikedViewModel by lazy {
        ViewModelProvider(this)[LikedViewModel::class.java]
    }

    override fun onCreateView(i: LayoutInflater, c: ViewGroup?, s: Bundle?): View =
        i.inflate(R.layout.fragment_liked, c, false)

    override fun onViewCreated(v: View, s: Bundle?) {
        val list: RecyclerView = v.findViewById(R.id.liked_list)
        list.layoutManager = LinearLayoutManager(context)
        val adapter = TrackAdapter(onPlay = { PlayerRepository.play(it, "Liked Songs") })
        list.adapter = adapter
        list.swipeToQueue(adapter)
        // Incremental loading at the tail.
        list.addOnScrollListener(object : RecyclerView.OnScrollListener() {
            override fun onScrolled(rv: RecyclerView, dx: Int, dy: Int) {
                val lm = rv.layoutManager as LinearLayoutManager
                if (lm.findLastVisibleItemPosition() >= adapter.itemCount - 4) vm.loadMore()
            }
        })
        v.findViewById<Button>(R.id.liked_play)?.setOnClickListener {
            adapter.currentList.firstOrNull()?.let { PlayerRepository.play(it, "Liked Songs") }
        }
        viewLifecycleOwner.lifecycleScope.launch {
            vm.state.collect { bindState(v, it) }
        }
        viewLifecycleOwner.lifecycleScope.launch {
            vm.tracks.collect { adapter.submitList(it) }
        }
        if (s == null) vm.load()
    }
}

// -- Queue (local player state only, no network) ----------------------------------------------
class QueueFragment : Fragment() {
    override fun onCreateView(i: LayoutInflater, c: ViewGroup?, s: Bundle?): View =
        i.inflate(R.layout.fragment_queue, c, false)

    override fun onViewCreated(v: View, s: Bundle?) {
        val now: TextView = v.findViewById(R.id.queue_now)
        val list: RecyclerView = v.findViewById(R.id.queue_list)
        list.layoutManager = LinearLayoutManager(context)
        // Unified timeline (Echo single-list queue): past + NOW + upcoming
        // in one draggable list; NOW is pinned (no handle) and tap-toggles.
        // Swipe removes with Undo (Echo dismiss); the clears empty each side.
        val adapter = QueueTimelineAdapter(
            onTapNext = { PlayerRepository.seekTimelinePosition(it) },
            onTapPast = { PlayerRepository.seekTimelinePosition(it) },
            onTapNow = { PlayerRepository.toggle() },
        )
        list.adapter = adapter
        list.queueDrag(adapter)
        list.swipeToRemove(adapter) { entry, pos ->
            PlayerRepository.deleteTimelineEntry(entry)
            com.google.android.material.snackbar.Snackbar.make(
                v,
                R.string.removed_from_queue,
                com.google.android.material.snackbar.Snackbar.LENGTH_LONG,
            ).setAction(R.string.action_undo) {
                PlayerRepository.insertTimelineEntry(entry, pos)
            }.show()
        }
        v.findViewById<Button>(R.id.queue_clear)?.setOnClickListener {
            PlayerRepository.clearQueue()
        }
        v.findViewById<Button>(R.id.history_clear)?.setOnClickListener {
            PlayerRepository.clearHistory()
        }
        var scrolledToNow = false
        viewLifecycleOwner.lifecycleScope.launch {
            PlayerRepository.state.collect { s ->
                now.text = s.track?.let { "Now playing: ${it.name} — ${it.artistNames}" }
                    ?: "Queue is empty"
                adapter.setTimeline(PlayerRepository.timeline())
                // Echo auto-scroll: land on the current row on open.
                if (!scrolledToNow) {
                    val nowPos = adapter.nowPosition()
                    if (nowPos >= 0) {
                        scrolledToNow = true
                        list.scrollToPosition(nowPos)
                    }
                }
                bindState(
                    v,
                    ScreenState.Content(
                        empty = s.queue.isEmpty() && s.track == null && s.history.isEmpty(),
                    ),
                )
            }
        }
    }
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
        val list: RecyclerView = v.findViewById(R.id.detail_list)
        list.layoutManager = LinearLayoutManager(context)
        val adapter = TrackAdapter(onPlay = { PlayerRepository.play(it, title.text.toString()) })
        list.adapter = adapter
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
        sortBtn?.setOnClickListener { anchor ->
            val menu = android.widget.PopupMenu(context, anchor)
            DetailViewModel.SortOrder.entries.forEachIndexed { i, order ->
                menu.menu.add(0, i, i, order.label).isCheckable = true
            }
            menu.menu.setGroupCheckable(0, true, true)
            menu.menu.getItem(vm.sort.value.ordinal)?.isChecked = true
            menu.setOnMenuItemClickListener { item ->
                DetailViewModel.SortOrder.entries.getOrNull(item.itemId)
                    ?.let { vm.setSort(it) }
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
            vm.tracks.collect { adapter.submitList(it) }
        }
        if (s == null) {
            val kind = arguments?.getString("kind", "playlist") ?: "playlist"
            val id = arguments?.getString("id", "") ?: ""
            title.text = arguments?.getString("title", "") ?: ""
            vm.load(kind, id)
        }
    }
}
