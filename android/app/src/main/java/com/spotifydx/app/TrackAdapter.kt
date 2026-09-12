package com.spotifydx.app

import android.annotation.SuppressLint
import android.view.LayoutInflater
import android.view.View
import android.view.ViewGroup
import android.widget.TextView
import androidx.recyclerview.widget.DiffUtil
import androidx.recyclerview.widget.ListAdapter
import androidx.recyclerview.widget.RecyclerView

/**
 * Track rows (shared primitive, §10.2 migration): artwork-backed title +
 * artist subtitle + duration label, tap-to-play with the in-hand [Track]
 * object (zero network, §9.1). Region-blocked null tracks never reach the
 * adapter — repositories filter them; the header/row grid stays aligned by
 * always emitting the index spacer (see `track-row--noindex` parity note).
 *
 * Queueing is a swipe gesture ([swipeToQueue], Spotify parity), not a
 * long-press — attach it wherever a TrackAdapter is bound.
 *
 * Drag handles: [onHandleTouch], armed by [queueDrag] — rows show the
 * handle only on reorderable (queue timeline) lists. The timeline itself
 * lives in [QueueTimelineAdapter]; this adapter stays a plain track list.
 */
class TrackAdapter(
    private val showIndex: Boolean = true,
    private val onPlay: (Track) -> Unit = {},
) : ListAdapter<Track, TrackAdapter.Holder>(DIFF) {

    companion object {
        val DIFF = object : DiffUtil.ItemCallback<Track>() {
            override fun areItemsTheSame(a: Track, b: Track): Boolean = a.id == b.id
            override fun areContentsTheSame(a: Track, b: Track): Boolean = a == b
        }

        fun formatDuration(ms: Long): String {
            if (ms <= 0) return "0:00"
            val s = (ms / 1000).toInt()
            return "%d:%02d".format(s / 60, s % 60)
        }
    }

    /**
     * Drag starter, armed by [queueDrag]: when non-null the row's drag
     * handle shows and touching it begins an instant drag (Echo's
     * `draggableHandle()`). Plain lists leave this null and show no handle.
     */
    var onHandleTouch: ((RecyclerView.ViewHolder) -> Unit)? = null

    inner class Holder(v: View) : RecyclerView.ViewHolder(v) {
        private val index: TextView = v.findViewById(R.id.track_index)
        private val art: android.widget.ImageView = v.findViewById(R.id.track_art)
        private val title: TextView = v.findViewById(R.id.track_title)
        private val subtitle: TextView = v.findViewById(R.id.track_subtitle)
        private val duration: TextView = v.findViewById(R.id.track_duration)
        private val handle: android.widget.ImageView = v.findViewById(R.id.track_handle)

        @SuppressLint("ClickableViewAccessibility")
        fun bind(t: Track, pos: Int) {
            if (showIndex) {
                index.visibility = View.VISIBLE
                index.text = (pos + 1).toString()
            } else {
                index.visibility = View.GONE
            }
            art.setTag(R.id.track_art, t.coverUrl)
            ArtworkLoader.load(art, t.coverUrl)
            title.text = t.name.ifEmpty { "Unknown track" }
            val sub = listOf(t.artistNames, t.albumName).filter { it.isNotEmpty() }
            subtitle.text = sub.joinToString(" · ")
            duration.text = formatDuration(t.durationMs)
            // Echo drag handle: visible only on reorderable lists; touching
            // it starts the drag instantly (consumed, so no tap-through play).
            val starter = onHandleTouch
            if (starter == null) {
                handle.visibility = View.GONE
                handle.setOnTouchListener(null)
            } else {
                handle.visibility = View.VISIBLE
                handle.setOnTouchListener { _, e ->
                    if (e.action == android.view.MotionEvent.ACTION_DOWN) {
                        starter(this)
                        true
                    } else {
                        false
                    }
                }
            }
            itemView.setOnClickListener { onPlay(t) }
        }
    }

    override fun onCreateViewHolder(parent: ViewGroup, viewType: Int): Holder {
        val v = LayoutInflater.from(parent.context).inflate(R.layout.item_track, parent, false)
        return Holder(v)
    }

    override fun onBindViewHolder(holder: Holder, position: Int) {
        holder.bind(getItem(position), position)
    }
}
