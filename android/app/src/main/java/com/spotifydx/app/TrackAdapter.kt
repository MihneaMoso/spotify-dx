package com.spotifydx.app

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
 */
class TrackAdapter(
    private val showIndex: Boolean = true,
    private val onPlay: (Track) -> Unit = {},
    private val onEnqueue: ((Track) -> Unit)? = null,
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

    inner class Holder(v: View) : RecyclerView.ViewHolder(v) {
        private val index: TextView = v.findViewById(R.id.track_index)
        private val art: android.widget.ImageView = v.findViewById(R.id.track_art)
        private val title: TextView = v.findViewById(R.id.track_title)
        private val subtitle: TextView = v.findViewById(R.id.track_subtitle)
        private val duration: TextView = v.findViewById(R.id.track_duration)

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
            itemView.setOnClickListener { onPlay(t) }
            itemView.setOnLongClickListener {
                onEnqueue?.invoke(t)
                true
            }
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
