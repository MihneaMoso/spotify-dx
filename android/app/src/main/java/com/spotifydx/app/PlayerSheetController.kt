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
    private lateinit var scrub: SeekBar
    private lateinit var volume: SeekBar
    private lateinit var play: ImageButton
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
        container.visibility == View.VISIBLE

    /** Bind once from MainActivity.onCreate (after setContentView). */
    fun bind(root: View) {
        container = root.findViewById(R.id.player_sheet)
        art = root.findViewById(R.id.sheet_art)
        title = root.findViewById(R.id.sheet_title)
        subtitle = root.findViewById(R.id.sheet_subtitle)
        source = root.findViewById(R.id.sheet_source)
        tier = root.findViewById(R.id.sheet_tier)
        pos = root.findViewById(R.id.sheet_pos)
        scrub = root.findViewById(R.id.sheet_scrub)
        volume = root.findViewById(R.id.sheet_volume)
        play = root.findViewById(R.id.sheet_play)
        main = root.findViewById(R.id.sheet_main)
        queueList = root.findViewById(R.id.sheet_queue)
        lyricsBox = root.findViewById(R.id.sheet_lyrics)
        lyricsList = root.findViewById(R.id.sheet_lyrics_list)
        lyricsPlainWrap = root.findViewById(R.id.sheet_lyrics_plain_wrap)
        lyricsPlain = root.findViewById(R.id.sheet_lyrics_plain)
        lyricsState = root.findViewById(R.id.sheet_lyrics_state)
        val tabQueue: Button = root.findViewById(R.id.sheet_tab_queue)
        val tabLyrics: Button = root.findViewById(R.id.sheet_tab_lyrics)

        root.findViewById<ImageButton>(R.id.sheet_minimize)?.setOnClickListener {
            close()
        }
        play.setOnClickListener { PlayerRepository.toggle() }
        root.findViewById<ImageButton>(R.id.sheet_next)?.setOnClickListener {
            PlayerRepository.nextTrack()
        }
        root.findViewById<ImageButton>(R.id.sheet_prev)?.setOnClickListener {
            PlayerRepository.seekTo(0)
        }
        scrub.setOnSeekBarChangeListener(seekListener { PlayerRepository.seekTo(it) })
        volume.setOnSeekBarChangeListener(seekListener { PlayerRepository.setVolume(it / 100f) })

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
                            close()
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
                source.text = st.source.ifEmpty { "" }
                source.visibility = if (st.source.isEmpty()) View.GONE else View.VISIBLE
                tier.text = st.audioTier
                tier.visibility = if (st.audioTier.isEmpty()) View.GONE else View.VISIBLE
                if (t == null) {
                    art.setImageDrawable(null)
                    lyricsTrackId = null
                    lyricsUi = LyricsUi.IDLE
                    renderLyrics()
                } else {
                    ArtworkLoader.load(art, t.coverUrl)
                    if (lyricsTrackId != t.id) {
                        lyricsTrackId = t.id
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
                pos.text = TrackAdapter.formatDuration(st.positionMs) +
                    " / " + TrackAdapter.formatDuration(st.durationMs)
                if (!volume.isPressed) {
                    volume.progress = (st.volume * 100).toInt()
                }
                queueAdapter.submitList(st.queue)
            }
        }
    }

    /** Slide the sheet up into view (idempotent). */
    fun open() {
        if (isOpen) return
        container.visibility = View.VISIBLE
        container.post {
            container.translationY = container.height.toFloat()
            container.animate().translationY(0f).setDuration(250).start()
        }
    }

    /** Slide down and hide (idempotent). */
    fun close() {
        if (!isOpen) return
        container.animate().translationY(container.height.toFloat())
            .setDuration(200)
            .withEndAction {
                container.visibility = View.GONE
                container.translationY = 0f
            }.start()
    }

    private fun showTab(t: Tab) {
        // Toggle tabs: tapping the active one returns to the player.
        tab = if (t == tab) Tab.MAIN else t
        main.visibility = if (tab == Tab.MAIN) View.VISIBLE else View.GONE
        queueList.visibility = if (tab == Tab.QUEUE) View.VISIBLE else View.GONE
        lyricsBox.visibility = if (tab == Tab.LYRICS) View.VISIBLE else View.GONE
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
                tv.setTextColor(
                    if (current) 0xFFFFFFFF.toInt() else 0xFF9AA6C3.toInt(),
                )
                tv.paint.isFakeBoldText = current
            }
        }
    }
}
