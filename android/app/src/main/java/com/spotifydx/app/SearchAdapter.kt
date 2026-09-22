package com.spotifydx.app

import android.view.LayoutInflater
import android.view.View
import android.view.ViewGroup
import android.widget.TextView
import androidx.recyclerview.widget.DiffUtil
import androidx.recyclerview.widget.ListAdapter
import androidx.recyclerview.widget.RecyclerView

/**
 * Unified search results: tracks, artists, and albums in one relevance-
 * ordered list (each array arrives Spotify-ordered; concatenated songs →
 * artists → albums with no client ranking). Track rows keep full
 * tap-to-play + swipe-to-queue; album/artist rows drill into detail.
 * Reuses the shared `item_track` / `item_title` layouts (same UI as
 * everywhere, art included).
 */
class SearchAdapter(
    private val onPlayTrack: (Track) -> Unit = {},
    private val onOpenAlbum: (id: String, name: String) -> Unit = { _, _ -> },
    private val onOpenArtist: (id: String, name: String) -> Unit = { _, _ -> },
    /** Context menu (2s hold or dots) with the row's resolved target. */
    private val onMenu: (MenuTarget) -> Unit = {},
) : ListAdapter<SearchRow, RecyclerView.ViewHolder>(DIFF) {

    companion object {
        private const val TYPE_TRACK = 0
        private const val TYPE_TITLE = 1

        private fun keyOf(r: SearchRow): String = when (r) {
            is SearchRow.TrackRow -> "t:${r.track.id}"
            is SearchRow.AlbumRow -> "a:${r.album.id}"
            is SearchRow.ArtistRow -> "r:${r.artist.id}"
        }

        val DIFF = object : DiffUtil.ItemCallback<SearchRow>() {
            override fun areItemsTheSame(a: SearchRow, b: SearchRow): Boolean =
                keyOf(a) == keyOf(b)
            override fun areContentsTheSame(a: SearchRow, b: SearchRow): Boolean = a == b
        }
    }

    inner class TrackHolder(v: View) : RecyclerView.ViewHolder(v) {
        private val art: android.widget.ImageView = v.findViewById(R.id.track_art)
        private val title: TextView = v.findViewById(R.id.track_title)
        private val subtitle: TextView = v.findViewById(R.id.track_subtitle)
        private val duration: TextView = v.findViewById(R.id.track_duration)
        private val explicitBadge: TextView = v.findViewById(R.id.track_explicit)

        init {
            // Search rows never show the index gutter.
            v.findViewById<View>(R.id.track_index)?.visibility = View.GONE
        }

        fun bind(t: Track) {
            art.setTag(R.id.track_art, t.coverUrl)
            ArtworkLoader.load(art, t.coverUrl)
            title.text = t.name.ifEmpty { "Unknown track" }
            subtitle.text = t.artistNames.ifEmpty { "Unknown artist" }
            duration.text = TrackAdapter.formatDuration(t.durationMs)
            TrackAdapter.bindExplicit(explicitBadge, t.explicit)
            itemView.setOnClickListener { onPlayTrack(t) }
            itemView.findViewById<android.widget.ImageButton>(R.id.track_more)
                ?.setOnClickListener { onMenu(MenuTarget.Song(t)) }
            HoldToOpen.arm(itemView) { onMenu(MenuTarget.Song(t)) }
        }
    }

    inner class TitleHolder(v: View) : RecyclerView.ViewHolder(v) {
        private val art: android.widget.ImageView = v.findViewById(R.id.title_art)
        private val main: TextView = v.findViewById(R.id.title_main)
        private val sub: TextView = v.findViewById(R.id.title_sub)

        fun bind(titleText: String, subtitleText: String, coverUrl: String) {
            art.setTag(R.id.title_art, coverUrl)
            ArtworkLoader.load(art, coverUrl)
            main.text = titleText.ifEmpty { "Unknown" }
            sub.text = subtitleText
            sub.visibility = if (subtitleText.isEmpty()) View.GONE else View.VISIBLE
        }
    }

    override fun getItemViewType(position: Int): Int = when (getItem(position)) {
        is SearchRow.TrackRow -> TYPE_TRACK
        else -> TYPE_TITLE
    }

    override fun onCreateViewHolder(parent: ViewGroup, viewType: Int): RecyclerView.ViewHolder {
        val inflater = LayoutInflater.from(parent.context)
        return if (viewType == TYPE_TRACK) {
            TrackHolder(inflater.inflate(R.layout.item_track, parent, false))
        } else {
            TitleHolder(inflater.inflate(R.layout.item_title, parent, false))
        }
    }

    override fun onBindViewHolder(holder: RecyclerView.ViewHolder, position: Int) {
        when (val row = getItem(position)) {
            is SearchRow.TrackRow -> (holder as TrackHolder).bind(row.track)
            is SearchRow.AlbumRow -> (holder as TitleHolder).apply {
                bind(row.album.name, row.album.artists.joinToString(", "), row.album.coverUrl)
                itemView.setOnClickListener { onOpenAlbum(row.album.id, row.album.name) }
                val target = MenuTarget.Album(row.album)
                itemView.findViewById<android.widget.ImageButton>(R.id.title_more)
                    ?.setOnClickListener { onMenu(target) }
                HoldToOpen.arm(itemView) { onMenu(target) }
            }
            is SearchRow.ArtistRow -> (holder as TitleHolder).apply {
                bind(row.artist.name, "Artist", row.artist.imageUrl)
                itemView.setOnClickListener { onOpenArtist(row.artist.id, row.artist.name) }
                val target = MenuTarget.Artist(row.artist)
                itemView.findViewById<android.widget.ImageButton>(R.id.title_more)
                    ?.setOnClickListener { onMenu(target) }
                HoldToOpen.arm(itemView) { onMenu(target) }
            }
        }
    }
}
