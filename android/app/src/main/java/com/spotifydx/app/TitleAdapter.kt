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
 */
class TitleAdapter(
    private val onClick: (Int) -> Unit = {},
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
        }
    }

    override fun onCreateViewHolder(parent: ViewGroup, viewType: Int): Holder {
        val v = LayoutInflater.from(parent.context).inflate(R.layout.item_title, parent, false)
        return Holder(v)
    }

    override fun onBindViewHolder(holder: Holder, position: Int) {
        holder.bind(getItem(position), position)
    }
}
