package com.spotifydx.app

import android.view.LayoutInflater
import android.view.View
import android.view.ViewGroup
import android.widget.TextView
import androidx.recyclerview.widget.DiffUtil
import androidx.recyclerview.widget.ListAdapter
import androidx.recyclerview.widget.RecyclerView

/**
 * Generic title/subtitle rows for shelves and collection tabs (playlists,
 * albums, artists). Track rows use [TrackAdapter]; detail navigation is the
 * caller's intent.
 *
 * @param itemLayout row layout; defaults to the full-width [R.layout.item_title]
 *   row. Horizontal shelves (Home playlists, desktop .shelf-row parity) pass
 *   [R.layout.item_card] — same view IDs, so binding is shared.
 */
class TitleAdapter(
    private val onClick: (Int) -> Unit = {},
    private val itemLayout: Int = R.layout.item_title,
    /**
     * Context menu (2s hold or dots, card overlay included): position-based
     * like [onClick] — rows are lossy, so the caller resolves pos → object.
     * Fired with the *current* bindingAdapterPosition (guarded), never the
     * stale bind-time index.
     */
    private val onMenu: (Int) -> Unit = {},
) : ListAdapter<TitleAdapter.Row, TitleAdapter.Holder>(DIFF) {

    data class Row(val title: String, val subtitle: String, val coverUrl: String = "")

    companion object {
        val DIFF = object : DiffUtil.ItemCallback<Row>() {
            override fun areItemsTheSame(a: Row, b: Row): Boolean =
                a.title == b.title && a.subtitle == b.subtitle
            override fun areContentsTheSame(a: Row, b: Row): Boolean = a == b
        }
    }

    inner class Holder(v: View) : RecyclerView.ViewHolder(v) {
        private val art: android.widget.ImageView = v.findViewById(R.id.title_art)
        private val main: TextView = v.findViewById(R.id.title_main)
        private val sub: TextView = v.findViewById(R.id.title_sub)

        fun bind(r: Row, pos: Int) {
            art.setTag(R.id.title_art, r.coverUrl)
            ArtworkLoader.load(art, r.coverUrl)
            main.text = r.title.ifEmpty { "Unknown" }
            sub.text = r.subtitle
            sub.visibility = if (r.subtitle.isEmpty()) View.GONE else View.VISIBLE
            itemView.setOnClickListener { onClick(pos) }
            fun fireMenu() {
                val p = bindingAdapterPosition
                if (p != RecyclerView.NO_POSITION) onMenu(p)
            }
            itemView.findViewById<android.widget.ImageButton>(R.id.title_more)
                ?.setOnClickListener { fireMenu() }
            HoldToOpen.arm(itemView, ::fireMenu)
        }
    }

    override fun onCreateViewHolder(parent: ViewGroup, viewType: Int): Holder {
        val v = LayoutInflater.from(parent.context).inflate(itemLayout, parent, false)
        return Holder(v)
    }

    override fun onBindViewHolder(holder: Holder, position: Int) {
        holder.bind(getItem(position), position)
    }
}
