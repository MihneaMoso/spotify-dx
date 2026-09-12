package com.spotifydx.app

import android.annotation.SuppressLint
import android.view.LayoutInflater
import android.view.View
import android.view.ViewGroup
import android.widget.TextView
import androidx.recyclerview.widget.RecyclerView
import java.util.Collections

/**
 * The unified queue timeline list, rewritten from scratch on Echo Music's
 * queue architecture (`ui/player/Queue.kt`):
 *
 * - ONE list owns past + current + upcoming ([QueueEntry] windows), exactly
 *   like Echo's single LazyColumn over player timeline windows — never a
 *   pop-queue plus a detached history.
 * - The adapter OWNS its windows as a plain mutable list (Echo's
 *   `mutableQueueWindows`). No Differ, no async diffs, no parallel
 *   kind arrays, no deferred submits: every mutation swaps data and
 *   notifies in the same call on the main thread, so adapter positions
 *   are exact at every instant of a gesture. Repository state re-syncs
 *   the list only when idle ([setTimeline] while dragging stashes and
 *   applies on drop — Echo's `LaunchedEffect(queueWindows)` resync).
 * - Taps follow Echo: PAST seeks back, NEXT seeks forward (both preserve
 *   the full timeline around the new current — see
 *   `PlayerRepository.seekTimelinePosition`), NOW toggles playback.
 * - NOW is highlighted and never swipe-dismissed, but drags like every
 *   other row: the drop commit re-splits past/upcoming around the current
 *   track's id, so the playing object is untouched and cross-current
 *   drags cannot disturb playback (Echo's rapid song-switching on such
 *   moves, excluded by construction).
 *
 * Rows reuse `item_track.xml` (artwork, title, subtitle, duration, drag
 * handle) — same UI as everywhere, Echo trailing-handle treatment.
 */
class QueueTimelineAdapter(
    private val onTapNext: (Int) -> Unit,
    private val onTapPast: (Int) -> Unit,
    private val onTapNow: () -> Unit,
) : RecyclerView.Adapter<QueueTimelineAdapter.Holder>() {

    /** Adapter-owned truth (Echo `mutableQueueWindows`). */
    private val windows = mutableListOf<QueueEntry>()

    /** True between drag-select and drop (resyncs defer meanwhile). */
    var dragging: Boolean = false
        private set

    /** Stashed repo state landing mid-drag (applied on drop, kinds intact). */
    private var pending: List<QueueEntry>? = null

    /**
     * Drag starter, armed by [queueDrag]: PAST/NEXT rows show the drag
     * handle and touching it begins an instant drag (Echo's
     * `draggableHandle()`).
     */
    var onHandleTouch: ((RecyclerView.ViewHolder) -> Unit)? = null

    /** Repository resync (Echo `LaunchedEffect(queueWindows)`). */
    fun setTimeline(entries: List<QueueEntry>) {
        if (dragging) {
            pending = entries
            return
        }
        // Ticker-speed emits with identical content skip silently — no
        // rebind churn while playing.
        if (entries == windows) return
        windows.clear()
        windows.addAll(entries)
        notifyDataSetChanged()
    }

    fun kindAt(pos: Int): RowKind = windows.getOrNull(pos)?.kind ?: RowKind.NEXT

    fun nowPosition(): Int = windows.indexOfFirst { it.kind == RowKind.NOW }

    fun entryAt(pos: Int): QueueEntry? = windows.getOrNull(pos)

    /** Drag session start (called by the drag helper on select). */
    fun beginDrag() {
        dragging = true
    }

    /**
     * Synchronous visual move: swaps entries AND rows in one call, so all
     * positions stay exact for the rest of the gesture.
     */
    fun swapWindows(from: Int, to: Int): Boolean {
        if (from !in windows.indices || to !in windows.indices) return false
        Collections.swap(windows, from, to)
        notifyItemMoved(from, to)
        return true
    }

    /** Drop: applies any stashed resync, returns the landed timeline. */
    fun endDrag(): List<QueueEntry> {
        dragging = false
        pending?.let {
            pending = null
            setTimeline(it)
        }
        return windows.toList()
    }

    /** Swipe-remove support (Echo dismiss): excises + animates one row. */
    fun removeAt(pos: Int): QueueEntry? {
        if (pos !in windows.indices) return null
        val removed = windows.removeAt(pos)
        notifyItemRemoved(pos)
        return removed
    }

    /** Undo support: splices an entry back at its old slot. */
    fun insertAt(pos: Int, entry: QueueEntry) {
        windows.add(pos.coerceIn(0, windows.size), entry)
        notifyItemInserted(pos.coerceIn(0, windows.size - 1))
    }

    override fun getItemCount(): Int = windows.size

    inner class Holder(v: View) : RecyclerView.ViewHolder(v) {
        private val index: TextView = v.findViewById(R.id.track_index)
        private val art: android.widget.ImageView = v.findViewById(R.id.track_art)
        private val title: TextView = v.findViewById(R.id.track_title)
        private val subtitle: TextView = v.findViewById(R.id.track_subtitle)
        private val duration: TextView = v.findViewById(R.id.track_duration)
        private val handle: android.widget.ImageView = v.findViewById(R.id.track_handle)

        @SuppressLint("ClickableViewAccessibility")
        fun bind(e: QueueEntry, pos: Int) {
            val t = e.track
            index.visibility = View.VISIBLE
            index.text = (pos + 1).toString()
            art.setTag(R.id.track_art, t.coverUrl)
            ArtworkLoader.load(art, t.coverUrl)
            title.text = t.name.ifEmpty { "Unknown track" }
            val sub = listOf(t.artistNames, t.albumName).filter { it.isNotEmpty() }
            subtitle.text = sub.joinToString(" · ")
            duration.text = TrackAdapter.formatDuration(t.durationMs)
            when (e.kind) {
                // NOW: highlighted, tap toggles (Echo: tapping the current
                // row toggles play/pause). Draggable like every other row —
                // the commit re-splits around the current track id, so the
                // player object is never touched by reorder.
                RowKind.NOW -> {
                    itemView.setBackgroundResource(R.drawable.row_playing)
                    armHandle()
                    itemView.setOnClickListener { onTapNow() }
                }
                else -> {
                    itemView.background = null
                    armHandle()
                    itemView.setOnClickListener {
                        val p = bindingAdapterPosition
                        if (p == RecyclerView.NO_POSITION) return@setOnClickListener
                        if (kindAt(p) == RowKind.PAST) onTapPast(p) else onTapNext(p)
                    }
                }
            }
        }

        // Echo drag handle: touching it starts the drag instantly
        // (consumed, so no tap-through play).
        private fun armHandle() {
            val starter = onHandleTouch
            if (starter == null) {
                handle.visibility = View.GONE
                handle.setOnTouchListener(null)
            } else {
                handle.visibility = View.VISIBLE
                handle.setOnTouchListener { _, ev ->
                    if (ev.action == android.view.MotionEvent.ACTION_DOWN) {
                        starter(this)
                        true
                    } else {
                        false
                    }
                }
            }
        }
    }

    override fun onCreateViewHolder(parent: ViewGroup, viewType: Int): Holder {
        val v = LayoutInflater.from(parent.context).inflate(R.layout.item_track, parent, false)
        return Holder(v)
    }

    override fun onBindViewHolder(holder: Holder, position: Int) {
        holder.bind(windows[position], position)
    }
}
