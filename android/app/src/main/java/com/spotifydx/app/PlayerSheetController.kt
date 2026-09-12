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
    private lateinit var line: TextView
    private lateinit var scrub: SeekBar
    private lateinit var play: ImageButton
    private lateinit var tabs: View
    private lateinit var spacer: View
    private lateinit var queueBox: View
    private lateinit var qThumb: ImageView
    private lateinit var qTitle: TextView
    private lateinit var qSub: TextView
    private lateinit var qCount: TextView
    private lateinit var tabQueue: Button
    private lateinit var tabLyrics: Button
    private lateinit var queueAdapter: TrackAdapter
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
        line = root.findViewById(R.id.sheet_line)
        queueBox = root.findViewById(R.id.sheet_queue_box)
        qThumb = root.findViewById(R.id.sheet_queue_thumb)
        qTitle = root.findViewById(R.id.sheet_queue_title)
        qSub = root.findViewById(R.id.sheet_queue_sub)
        qCount = root.findViewById(R.id.sheet_queue_count)
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
        play.setOnClickListener { PlayerRepository.toggle(); punch(play) }
        root.findViewById<ImageButton>(R.id.sheet_next)?.setOnClickListener {
            PlayerRepository.nextTrack(); punch(it)
        }
        root.findViewById<ImageButton>(R.id.sheet_prev)?.setOnClickListener {
            PlayerRepository.seekTo(0); punch(it)
        }
        // Echo transport feel: press sinks to 0.88 with a springy release.
        // Purely visual touch feedback — click behavior is untouched.
        root.findViewById<ImageButton>(R.id.sheet_next)?.let(::pressScale)
        root.findViewById<ImageButton>(R.id.sheet_prev)?.let(::pressScale)
        pressScale(play)
        scrub.setOnSeekBarChangeListener(seekListener { PlayerRepository.seekTo(it) })

        queueList.layoutManager = LinearLayoutManager(activity)
        queueAdapter = TrackAdapter(onPlay = { PlayerRepository.play(it, "Queue") })
        queueList.adapter = queueAdapter
        queueList.swipeToQueue(queueAdapter)
        queueList.queueDrag(queueAdapter)

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
                    line.visibility = View.GONE
                    lyricsTrackId = null
                    lyricsUi = LyricsUi.IDLE
                    renderLyrics()
                } else {
                    ArtworkLoader.load(art, t.coverUrl, ArtworkLoader.Art.PLAYER)
                    if (lyricsTrackId != t.id) {
                        lyricsTrackId = t.id
                        line.visibility = View.GONE
                        loadLyrics(t)
                    }
                    updateHighlight(st.positionMs)
                }
                play.setImageResource(
                    if (st.isPlaying) android.R.drawable.ic_media_pause
                    else android.R.drawable.ic_media_play,
                )
                if (st.durationMs > 0) {
                    scrub.max = st.durationMs.toInt()
                    if (!scrub.isPressed) scrub.progress = st.positionMs.toInt()
                }
                pos.text = TrackAdapter.formatDuration(st.positionMs)
                duration.text = TrackAdapter.formatDuration(st.durationMs)
                queueAdapter.submitList(st.queue)
                // Echo queue header: current track + queue size.
                qTitle.text = t?.name ?: "Not playing"
                qSub.text = t?.artistNames ?: ""
                qCount.text = "${st.queue.size} songs"
                if (t != null) ArtworkLoader.load(qThumb, t.coverUrl)
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
        // One-liner only exists for synced lyrics; sections own the rest.
        if (!showList) line.visibility = View.GONE
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
        lyricsList.scrollToPosition(idx)
        // Echo synced one-liner under the artwork.
        if (idx in lyricsLines.indices) {
            line.text = lyricsLines[idx].text
            if (line.visibility != View.VISIBLE) line.visibility = View.VISIBLE
        }
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
