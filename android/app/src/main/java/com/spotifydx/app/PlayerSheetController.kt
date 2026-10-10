package com.spotifydx.app

import android.view.View
import android.view.ViewGroup
import android.widget.Button
import android.widget.ImageButton
import android.widget.ImageView
import android.widget.SeekBar
import android.widget.TextView
import androidx.fragment.app.FragmentActivity
import androidx.lifecycle.lifecycleScope
import androidx.recyclerview.widget.LinearLayoutManager
import androidx.recyclerview.widget.RecyclerView
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * Full-screen player sheet as a persistent overlay in the activity layout
 * (Echo Music architecture — deliberately NOT a DialogFragment, whose
 * window brought theme/token issues and rendered empty).
 *
 * Fed by the same [PlayerRepository.state] flow as the mini player bar.
 * Slide-up opens ([open]), chevron / system back / swipe-down minimize
 * ([close]). Tabs toggle the lower content: Queue (drag-reorderable) and
 * Lyrics (synced highlight + auto-scroll, four states).
 */
class PlayerSheetController(private val activity: FragmentActivity) {
    private enum class Tab { MAIN, QUEUE, LYRICS }

    private enum class LyricsUi {
        LOADING, SYNCED, PLAIN, INSTRUMENTAL, UNAVAILABLE, IDLE,
    }

    private data class LyricsData(
        val lines: List<LrcLine>,
        val plain: String,
        val instrumental: Boolean,
    )

    private lateinit var container: View
    private lateinit var main: View
    private lateinit var queueList: RecyclerView
    private lateinit var lyricsBox: View
    private lateinit var lyricsList: RecyclerView
    private lateinit var lyricsPlainWrap: View
    private lateinit var lyricsPlain: TextView
    private lateinit var lyricsState: TextView
    private lateinit var art: ImageView
    private lateinit var title: TextView
    private lateinit var subtitle: TextView
    private lateinit var source: TextView
    private lateinit var tier: TextView
    private lateinit var pos: TextView
    private lateinit var duration: TextView
    private lateinit var previewBox: View
    private lateinit var previewRows: List<TextView>
    private lateinit var scrub: SeekBar
    private lateinit var play: ImageButton
    private lateinit var tabs: View
    private lateinit var spacer: View
    private lateinit var queueBox: View
    private lateinit var topCount: TextView
    private lateinit var miniArt: ImageView
    private lateinit var miniTitle: TextView
    private lateinit var miniSub: TextView
    private lateinit var miniPlay: ImageButton
    private lateinit var miniScrub: SeekBar
    private lateinit var miniPos: TextView
    private lateinit var miniDur: TextView
    private lateinit var tabQueue: Button
    private lateinit var tabLyrics: Button
    private lateinit var queueAdapter: QueueTimelineAdapter
    private lateinit var lyricsAdapter: LyricsAdapter

    private var tab: Tab = Tab.MAIN

    /** Per-track lyrics cache (controller lifetime — the core caches beyond). */
    private val lyricsCache = mutableMapOf<String, LyricsData>()

    private var lyricsLines: List<LrcLine> = emptyList()
    private var lyricsIndex: Int = -1
    private var lyricsTrackId: String? = null
    private var lyricsUi: LyricsUi = LyricsUi.IDLE

    val isOpen: Boolean get() = ::container.isInitialized &&
        container.visibility != View.GONE

    /** Bind once from MainActivity.onCreate (after setContentView). */
    fun bind(root: View) {
        container = root.findViewById(R.id.player_sheet)
        art = root.findViewById(R.id.sheet_art)
        title = root.findViewById(R.id.sheet_title)
        subtitle = root.findViewById(R.id.sheet_subtitle)
        source = root.findViewById(R.id.sheet_source)
        tier = root.findViewById(R.id.sheet_tier)
        pos = root.findViewById(R.id.sheet_pos)
        duration = root.findViewById(R.id.sheet_duration)
        previewBox = root.findViewById(R.id.sheet_lyrics_preview)
        previewRows = listOf(
            root.findViewById(R.id.sheet_lp2),
            root.findViewById(R.id.sheet_lp1),
            root.findViewById(R.id.sheet_lcur),
            root.findViewById(R.id.sheet_ln1),
            root.findViewById(R.id.sheet_ln2),
        )
        // Preview height ≈ 1/5 of the screen (user ask: a sixth to a
        // fifth), clamped so small screens keep the transport reachable
        // and large screens don't bloat. XML default covers first layout.
        val dm = activity.resources.displayMetrics
        val fifth = (dm.heightPixels / 5f / dm.density).toInt()
        val boxDp = fifth.coerceIn(112, 160)
        previewBox.layoutParams = previewBox.layoutParams.apply {
            height = (boxDp * dm.density).toInt()
        }
        queueBox = root.findViewById(R.id.sheet_queue_box)
        topCount = root.findViewById(R.id.sheet_queue_count_top)
        miniArt = root.findViewById(R.id.sheet_mini_art)
        miniTitle = root.findViewById(R.id.sheet_mini_title)
        miniSub = root.findViewById(R.id.sheet_mini_subtitle)
        miniPlay = root.findViewById(R.id.sheet_mini_play)
        miniScrub = root.findViewById(R.id.sheet_mini_scrub)
        miniPos = root.findViewById(R.id.sheet_mini_pos)
        miniDur = root.findViewById(R.id.sheet_mini_duration)
        scrub = root.findViewById(R.id.sheet_scrub)
        play = root.findViewById(R.id.sheet_play)
        tabs = root.findViewById(R.id.sheet_tabs)
        spacer = root.findViewById(R.id.sheet_spacer)
        main = root.findViewById(R.id.sheet_main)
        queueList = root.findViewById(R.id.sheet_queue)
        lyricsBox = root.findViewById(R.id.sheet_lyrics)
        lyricsList = root.findViewById(R.id.sheet_lyrics_list)
        lyricsPlainWrap = root.findViewById(R.id.sheet_lyrics_plain_wrap)
        lyricsPlain = root.findViewById(R.id.sheet_lyrics_plain)
        lyricsState = root.findViewById(R.id.sheet_lyrics_state)
        tabQueue = root.findViewById(R.id.sheet_tab_queue)
        tabLyrics = root.findViewById(R.id.sheet_tab_lyrics)

        root.findViewById<ImageButton>(R.id.sheet_minimize)?.setOnClickListener {
            close()
        }
        // Same song menu as every row's dots (queue, lists, cards): the
        // current track's details dialog. No-op with no track loaded.
        root.findViewById<ImageButton>(R.id.sheet_more)?.setOnClickListener {
            PlayerRepository.state.value.track?.let {
                ContextMenuHost.showMenu(activity.supportFragmentManager, MenuTarget.Song(it))
            }
        }
        play.setOnClickListener { PlayerRepository.toggle(); punch(play) }
        root.findViewById<ImageButton>(R.id.sheet_mini_play)?.setOnClickListener {
            PlayerRepository.toggle()
        }
        root.findViewById<ImageButton>(R.id.sheet_mini_next)?.setOnClickListener {
            PlayerRepository.nextTrack()
        }
        root.findViewById<ImageButton>(R.id.sheet_mini_prev)?.setOnClickListener {
            PlayerRepository.previousTrack()
        }
        miniScrub.setOnSeekBarChangeListener(seekListener { PlayerRepository.seekTo(it) })
        root.findViewById<ImageButton>(R.id.sheet_next)?.setOnClickListener {
            PlayerRepository.nextTrack(); punch(it)
        }
        root.findViewById<ImageButton>(R.id.sheet_prev)?.setOnClickListener {
            PlayerRepository.previousTrack(); punch(it)
        }
        // Echo transport feel: press sinks to 0.88 with a springy release.
        // Purely visual touch feedback — click behavior is untouched.
        root.findViewById<ImageButton>(R.id.sheet_next)?.let(::pressScale)
        root.findViewById<ImageButton>(R.id.sheet_prev)?.let(::pressScale)
        pressScale(play)
        scrub.setOnSeekBarChangeListener(seekListener { PlayerRepository.seekTo(it) })

        queueList.layoutManager = LinearLayoutManager(activity)
        queueAdapter = QueueTimelineAdapter(
            onTapNext = { PlayerRepository.seekTimelinePosition(it) },
            onTapPast = { PlayerRepository.seekTimelinePosition(it) },
            onTapNow = { PlayerRepository.restartCurrent() },
            onMenu = {
                ContextMenuHost.showMenu(activity.supportFragmentManager, MenuTarget.Song(it))
            },
        )
        queueList.adapter = queueAdapter
        queueList.queueDrag(queueAdapter)
        queueList.swipeToRemove(queueAdapter) { entry, pos ->
            PlayerRepository.deleteTimelineEntryAt(entry, pos)
            com.google.android.material.snackbar.Snackbar.make(
                container,
                R.string.removed_from_queue,
                com.google.android.material.snackbar.Snackbar.LENGTH_LONG,
            ).setAction(R.string.action_undo) {
                PlayerRepository.insertTimelineEntry(entry, pos)
            }.show()
        }

        lyricsList.layoutManager = LinearLayoutManager(activity)
        lyricsAdapter = LyricsAdapter()
        lyricsList.adapter = lyricsAdapter

        tabQueue.setOnClickListener { showTab(Tab.QUEUE) }
        tabLyrics.setOnClickListener { showTab(Tab.LYRICS) }

        // Swipe-down anywhere on non-interactive sheet areas minimizes —
        // same as the chevron / system back. Tracked MANUALLY (mirrors the
        // bar's swipe-up in MainActivity): a 150px downward run closes once.
        // Touches on buttons / seekbars / lists are consumed by those views
        // and never reach here, so scrolling the queue can't dismiss.
        var lastY = 0f
        var accDy = 0f
        container.setOnTouchListener { _, e ->
            when (e.action) {
                android.view.MotionEvent.ACTION_DOWN -> {
                    lastY = e.y
                    accDy = 0f
                    // Claim the stream so the MOVE run reaches us; only
                    // background touches arrive here (children consume
                    // their own), so nothing else needs them.
                    true
                }
                android.view.MotionEvent.ACTION_MOVE -> {
                    val dy = e.y - lastY
                    lastY = e.y
                    if (dy > 0) {
                        accDy += dy
                        if (accDy > 150) {
                            accDy = 0f
                            // Expanded Queue/Lyrics collapse first (Echo:
                            // inner sheet consumes the drag), then the sheet.
                            if (!backToMain()) close()
                        }
                    } else {
                        accDy = 0f
                    }
                    true
                }
                else -> false
            }
        }

        activity.lifecycleScope.launch {
            PlayerRepository.state.collect { st ->
                val t = st.track
                title.text = t?.name ?: "Not playing"
                subtitle.text = t?.artistNames ?: ""
                // Echo "playing from": album context, provider only as fallback.
                val from = t?.albumName?.ifEmpty { null } ?: st.source
                source.text = from
                source.visibility = if (from.isEmpty()) View.GONE else View.VISIBLE
                tier.text = st.audioTier
                tier.visibility = if (st.audioTier.isEmpty()) View.GONE else View.VISIBLE
                if (t == null) {
                    art.setImageDrawable(null)
                    previewBox.visibility = View.GONE
                    lyricsTrackId = null
                    lyricsUi = LyricsUi.IDLE
                    renderLyrics()
                } else {
                    // Tag-guarded like the mini player: this collector runs
                    // on every position tick, and an unguarded load()
                    // restarts the Coil request continuously.
                    if (art.getTag(R.id.sheet_art) != t.coverUrl) {
                        art.setTag(R.id.sheet_art, t.coverUrl)
                        ArtworkLoader.load(art, t.coverUrl, ArtworkLoader.Art.PLAYER)
                    }
                    if (lyricsTrackId != t.id) {
                        lyricsTrackId = t.id
                        previewBox.visibility = View.GONE
                        loadLyrics(t)
                    }
                    updateHighlight(st.positionMs)
                }
                if (play.getTag(R.id.sheet_play) != st.isPlaying) {
                    play.setTag(R.id.sheet_play, st.isPlaying)
                    play.setImageResource(
                        if (st.isPlaying) android.R.drawable.ic_media_pause
                        else android.R.drawable.ic_media_play,
                    )
                }
                if (st.durationMs > 0) {
                    scrub.max = st.durationMs.toInt()
                    if (!scrub.isPressed) scrub.progress = st.positionMs.toInt()
                } else {
                    // No duration yet (track-switch gap) or unknown: never
                    // show the previous track's max/progress under the new
                    // title. Guarded writes (this runs per position tick).
                    if (scrub.max != 0) scrub.max = 0
                    if (scrub.progress != 0) scrub.progress = 0
                }
                pos.text = TrackAdapter.formatDuration(st.positionMs)
                duration.text = TrackAdapter.formatDuration(st.durationMs)
                queueAdapter.setTimeline(PlayerRepository.timeline())
                // In-queue mini player: same transport as the floating bar.
                renderSheetMini(st)
                // Queue size lives in the top menu now, left of the dots.
                val count = topCount.context.getString(R.string.queue_count, st.queue.size)
                if (topCount.text.toString() != count) topCount.text = count
            }
        }
    }

    /** Slide the sheet up into view (idempotent). */
    fun open() {
        if (isOpen) return
        // INVISIBLE (not GONE) first: the sheet measures and lays out with
        // zero frames flashed at rest position. The posted block then parks
        // it below the screen, flips it VISIBLE, and eases it up — one
        // continuous motion, no content flash. 350ms Decelerate ≈ Echo's
        // spring settle without new dependencies.
        container.visibility = View.INVISIBLE
        container.post {
            // A close() that landed in between wins — never re-show.
            if (container.visibility == View.GONE) return@post
            container.translationY = container.height.toFloat()
            container.visibility = View.VISIBLE
            container.animate().translationY(0f).setDuration(350)
                .setInterpolator(android.view.animation.DecelerateInterpolator())
                .start()
        }
    }

    /** Slide down and hide (idempotent). */
    fun close() {
        if (!isOpen) return
        container.animate().translationY(container.height.toFloat())
            .setDuration(300)
            .setInterpolator(android.view.animation.DecelerateInterpolator())
            .withEndAction {
                container.visibility = View.GONE
                container.translationY = 0f
            }.start()
    }

    /** Echo back order: expanded Queue/Lyrics collapse to the player first.
     *  Returns true when a section was collapsed (back press consumed). */
    fun backToMain(): Boolean {
        if (!isOpen || tab == Tab.MAIN) return false
        showTab(tab)
        return true
    }

    private fun showTab(t: Tab) {
        // Echo nested-sheet behavior: the Queue / Lyrics section expands over
        // the main player content with a fade+rise; tapping the active one
        // (or Back) returns to the player. Labels stay ours ("Queue").
        tab = if (t == tab) Tab.MAIN else t
        val showMain = tab == Tab.MAIN
        main.visibility = if (showMain) View.VISIBLE else View.GONE
        queueBox.visibility = if (tab == Tab.QUEUE) View.VISIBLE else View.GONE
        lyricsBox.visibility = if (tab == Tab.LYRICS) View.VISIBLE else View.GONE
        // The bottom Queue | Lyrics row + spacer belong to the player view;
        // the expanded section takes the whole sheet (Echo nested sheet).
        tabs.visibility = if (showMain) View.VISIBLE else View.GONE
        spacer.visibility = if (showMain) View.VISIBLE else View.GONE
        val shown: View = when (tab) {
            Tab.QUEUE -> queueBox
            Tab.LYRICS -> lyricsBox
            Tab.MAIN -> main
        }
        shown.alpha = 0f
        shown.translationY = 48f
        shown.animate().alpha(1f).translationY(0f).setDuration(200).start()
        // Echo split-button look: tab_bg/tab_text selectors react to selected.
        tabQueue.isSelected = tab == Tab.QUEUE
        tabLyrics.isSelected = tab == Tab.LYRICS
        // Echo auto-scroll: land on the current row when expanding Queue.
        if (tab == Tab.QUEUE) {
            val nowPos = queueAdapter.nowPosition()
            if (nowPos >= 0) queueList.scrollToPosition(nowPos)
        }
        // Queue size describes the queue view: only there.
        topCount.visibility = if (tab == Tab.QUEUE) View.VISIBLE else View.GONE
    }

    /** In-queue mini player state: mirrors the floating bar (titles, art,
     * transport icon, scrub, times). Compare-before-write throughout —
     * this runs on every position tick. */
    private fun renderSheetMini(s: PlayerRepository.State) {
        val title = s.track?.name?.ifEmpty { "Not playing" } ?: "Not playing"
        if (miniTitle.text.toString() != title) miniTitle.text = title
        val sub = s.track?.artistNames ?: ""
        if (miniSub.text.toString() != sub) miniSub.text = sub
        val url = s.track?.coverUrl ?: ""
        if (miniArt.getTag(R.id.sheet_mini_art) != url) {
            miniArt.setTag(R.id.sheet_mini_art, url)
            ArtworkLoader.load(miniArt, url, ArtworkLoader.Art.PLAYER)
        }
        if (miniPlay.getTag(R.id.sheet_mini_play) != s.isPlaying) {
            miniPlay.setTag(R.id.sheet_mini_play, s.isPlaying)
            miniPlay.setImageResource(
                if (s.isPlaying) android.R.drawable.ic_media_pause
                else android.R.drawable.ic_media_play,
            )
        }
        if (s.durationMs > 0) {
            val max = s.durationMs.toInt()
            if (miniScrub.max != max) miniScrub.max = max
            if (!miniScrub.isPressed) miniScrub.progress = s.positionMs.toInt()
        } else {
            if (miniScrub.max != 0) miniScrub.max = 0
            if (miniScrub.progress != 0) miniScrub.progress = 0
        }
        val pos = TrackAdapter.formatDuration(s.positionMs)
        if (miniPos.text.toString() != pos) miniPos.text = pos
        val dur = TrackAdapter.formatDuration(s.durationMs)
        if (miniDur.text.toString() != dur) miniDur.text = dur
    }

    /**
     * Echo transport feel, View edition. Echo presses its transport buttons
     * to 0.9 scale on a spring and morphs play while playing (WavyShape —
     * not expressible with framework drawables); the springy press + tap
     * punch carry the same feel with zero new dependencies.
     */
    private fun pressScale(v: View) {
        v.setOnTouchListener { _, e ->
            when (e.action) {
                android.view.MotionEvent.ACTION_DOWN -> {
                    v.animate().scaleX(0.88f).scaleY(0.88f)
                        .setDuration(100).start()
                    // false: the click listener still fires on UP.
                    false
                }
                android.view.MotionEvent.ACTION_UP,
                android.view.MotionEvent.ACTION_CANCEL -> {
                    v.animate().scaleX(1f).scaleY(1f).setDuration(150).start()
                    false
                }
                else -> false
            }
        }
    }

    /** Tap punch: quick sink-and-release on activation. */
    private fun punch(v: View) {
        v.animate().scaleX(0.85f).scaleY(0.85f).setDuration(80)
            .withEndAction {
                v.animate().scaleX(1f).scaleY(1f).setDuration(120).start()
            }.start()
    }

    private fun seekListener(onSeek: (Long) -> Unit) =
        object : SeekBar.OnSeekBarChangeListener {
            override fun onProgressChanged(sb: SeekBar?, p: Int, fromUser: Boolean) {
                if (fromUser) onSeek(p.toLong())
            }

            override fun onStartTrackingTouch(sb: SeekBar?) {}
            override fun onStopTrackingTouch(sb: SeekBar?) {}
        }

    private fun renderLyrics() {
        val showList = lyricsUi == LyricsUi.SYNCED
        val showPlain = lyricsUi == LyricsUi.PLAIN
        lyricsList.visibility = if (showList) View.VISIBLE else View.GONE
        lyricsPlainWrap.visibility = if (showPlain) View.VISIBLE else View.GONE
        lyricsState.visibility =
            if (!showList && !showPlain) View.VISIBLE else View.GONE
        // Preview window only exists for synced lyrics; sections own rest.
        if (!showList) previewBox.visibility = View.GONE
        if (!showList && !showPlain) {
            lyricsState.text = when (lyricsUi) {
                LyricsUi.LOADING -> "Loading lyrics…"
                LyricsUi.INSTRUMENTAL -> "Instrumental"
                LyricsUi.UNAVAILABLE -> "Lyrics not available"
                else -> ""
            }
        }
    }

    private fun applyLyricsData(data: LyricsData?) {
        lyricsIndex = -1
        lyricsLines = emptyList()
        lyricsUi = when {
            data == null -> LyricsUi.UNAVAILABLE
            data.instrumental -> LyricsUi.INSTRUMENTAL
            data.lines.isNotEmpty() -> {
                lyricsLines = data.lines
                lyricsAdapter.submitList(data.lines.map { it.text })
                LyricsUi.SYNCED
            }
            data.plain.isNotEmpty() -> {
                lyricsPlain.text = data.plain
                LyricsUi.PLAIN
            }
            else -> LyricsUi.UNAVAILABLE
        }
    }

    private fun loadLyrics(t: Track) {
        val id = t.id
        if (id.isEmpty()) {
            lyricsUi = LyricsUi.UNAVAILABLE
            renderLyrics()
            return
        }
        lyricsCache[id]?.let { cached ->
            applyLyricsData(cached)
            renderLyrics()
            return
        }
        lyricsUi = LyricsUi.LOADING
        renderLyrics()
        activity.lifecycleScope.launch {
            val json = withContext(Dispatchers.IO) {
                MusicRepository.lyrics(t).getOrNull()
            }
            // Stale response guard: track changed while fetching.
            if (lyricsTrackId != id) return@launch
            val data = if (json == null) {
                null
            } else {
                LyricsData(
                    lines = Lrc.parse(json.optString("synced", "")),
                    plain = json.optString("plain", ""),
                    instrumental = json.optBoolean("instrumental", false),
                )
            }
            if (data != null) lyricsCache[id] = data
            applyLyricsData(data)
            renderLyrics()
        }
    }

    private fun updateHighlight(positionMs: Long) {
        if (lyricsUi != LyricsUi.SYNCED || lyricsLines.isEmpty()) return
        val idx = Lrc.indexAt(lyricsLines, positionMs)
        if (idx == lyricsIndex) return
        val prev = lyricsIndex
        lyricsIndex = idx
        if (prev >= 0) lyricsAdapter.notifyItemChanged(prev)
        lyricsAdapter.notifyItemChanged(idx)
        // Auto-scroll only while the user isn't reading elsewhere: a reader
        // who dragged the list (non-idle scroll state) keeps their viewport
        // and still gets the highlight + preview updates.
        if (lyricsList.scrollState == RecyclerView.SCROLL_STATE_IDLE) {
            lyricsList.scrollToPosition(idx)
        }
        // Spotify-style preview window: previous two lines, current,
        // next two — wrapped, edge-faded, crossfaded on change.
        if (idx in lyricsLines.indices) {
            renderPreviewWindow(idx)
        }
    }

    /** Five-row preview around `idx` (missing edges stay empty; row
     * minHeights keep the geometry stable while sliding). Rows fade toward
     * the edges via static alphas (Spotify feel, theme-attr colors); the
     * whole window crossfades on each line change (simple Echo-style
     * alpha animation, 220ms). Compare-before-write upstream guarantees
     * this runs only on real line changes, never per tick. */
    private fun renderPreviewWindow(idx: Int) {
        for (r in -2..2) {
            val row = previewRows[r + 2]
            val text = lyricsLines.getOrNull(idx + r)?.text ?: ""
            if (row.text.toString() != text) row.text = text
        }
        if (previewBox.visibility != View.VISIBLE) {
            previewBox.visibility = View.VISIBLE
            previewBox.alpha = 0f
        } else {
            previewBox.alpha = 0.35f
        }
        previewBox.animate().cancel()
        previewBox.animate().alpha(1f).setDuration(220).start()
    }

    /**
     * Synced-lyrics lines: current line bright, the rest muted. The sheet
     * drives `notifyItemChanged` surgically (prev + new only) — never a
     * full rebind per tick.
     */
    inner class LyricsAdapter : RecyclerView.Adapter<LyricsAdapter.Holder>() {
        private var lines: List<String> = emptyList()

        fun submitList(next: List<String>) {
            lines = next
            notifyDataSetChanged()
        }

        override fun getItemCount(): Int = lines.size

        override fun onCreateViewHolder(parent: ViewGroup, viewType: Int): Holder {
            val tv = TextView(parent.context).apply {
                layoutParams = ViewGroup.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT,
                    ViewGroup.LayoutParams.WRAP_CONTENT,
                )
                val pad = (16 * resources.displayMetrics.density).toInt()
                setPadding(0, pad / 2, 0, pad / 2)
                textSize = 16f
                setLineSpacing(0f, 1.5f)
            }
            return Holder(tv)
        }

        override fun onBindViewHolder(holder: Holder, position: Int) {
            holder.bind(lines[position], position == lyricsIndex)
        }

        inner class Holder(private val tv: TextView) :
            RecyclerView.ViewHolder(tv) {
            fun bind(text: String, current: Boolean) {
                tv.text = text
                // Theme-attr lyric colors (were hardcoded white/muted hexes,
                // wrong whenever the palette shifts): active line reads on
                // the surface, idle lines recede to muted.
                tv.setTextColor(
                    Design.resolveAttr(
                        tv.context,
                        if (current) com.google.android.material.R.attr.colorOnSurface
                        else com.google.android.material.R.attr.colorOnSurfaceVariant,
                    ),
                )
                tv.paint.isFakeBoldText = current
            }
        }
    }
}
