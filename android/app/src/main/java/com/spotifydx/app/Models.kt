package com.spotifydx.app

import org.json.JSONArray
import org.json.JSONObject

/**
 * Typed models for tracks, albums, artists, playlists, and feeds (§7.3
 * ARCHITECTURE). All parsers tolerate partial payloads — missing fields
 * default instead of breaking — matching the core's tolerant models, since
 * the same internal-API response shapes cross the bridge as JSON.
 *
 * Field map (core `spotify::models`, serialized 1:1):
 * - Track: {id, name, artists:[{id, name}], album:{id, name, images:[{url,width}]},
 *   duration_ms, uri}
 * - Album: {id, name, artists, images, release_date, total_tracks, uri}
 * - Playlist (GQL): {id, name, images, tracks:{total, items:[Track]}, uri}
 * - ArtistPage: {artist, albums[], top_tracks[], related[]}
 * - SearchResults: {tracks/albums/artists:{items[], total}}
 * - SavedTrack: {track: {...}|null} — nulls are region-blocked skips.
 */
data class Track(
    val id: String = "",
    val name: String = "",
    val artists: List<String> = emptyList(),
    /** Parallel to [artists]: Spotify IDs for artist-page navigation. */
    val artistIds: List<String> = emptyList(),
    val albumName: String = "",
    /** Spotify album ID for album-page navigation; "" when unknown. */
    val albumId: String = "",
    val coverUrl: String = "",
    val durationMs: Long = 0,
    val uri: String = "",
    /** ISO-8601 playlist add time; "" when the source carries none. */
    val addedAt: String = "",
) {
    val artistNames: String get() = artists.joinToString(", ")
    val playable: Boolean get() = id.isNotEmpty() && name.isNotEmpty()
}

data class Album(
    val id: String = "",
    val name: String = "",
    val artists: List<String> = emptyList(),
    /** Parallel to [artists]: Spotify IDs for artist-page navigation. */
    val artistIds: List<String> = emptyList(),
    val coverUrl: String = "",
    val trackCount: Int = 0,
)

data class Artist(
    val id: String = "",
    val name: String = "",
    val imageUrl: String = "",
)

/**
 * One row of the unified queue timeline (Echo's single past+current+
 * upcoming list): the queue is presented — and reordered — as one list,
 * not as a pop-queue plus a detached history. NOW is the pinned anchor:
 * it never moves, so cross-current drags cannot disturb playback.
 */
enum class RowKind { PAST, NOW, NEXT }

data class QueueEntry(
    val track: Track,
    val kind: RowKind,
)

data class Playlist(
    val id: String = "",
    val name: String = "",
    val coverUrl: String = "",
    val trackCount: Int = 0,
)

/**
 * One row of the unified search results: the API returns tracks, artists,
 * and albums each internally relevance-ordered; the list concatenates them
 * in the official app's vertical section order (songs, artists, albums)
 * with no client-side ranking invented on top.
 */
sealed interface SearchRow {
    data class TrackRow(val track: Track) : SearchRow
    data class AlbumRow(val album: Album) : SearchRow
    data class ArtistRow(val artist: Artist) : SearchRow
}

/** Lenient readers over core JSON (per-operation field variants tolerated). */
object Models {
    /** Snapshot a [Track] to the core-shaped JSON [track] parses back —
     * queue / last-played persistence round-trips through this. */
    fun trackToJson(t: Track): org.json.JSONObject {
        val artists = org.json.JSONArray()
        t.artists.forEachIndexed { i, name ->
            artists.put(
                org.json.JSONObject()
                    .put("name", name)
                    .put("id", t.artistIds.getOrNull(i) ?: ""),
            )
        }
        val images = org.json.JSONArray()
        if (t.coverUrl.isNotEmpty()) {
            images.put(org.json.JSONObject().put("url", t.coverUrl))
        }
        return org.json.JSONObject()
            .put("id", t.id)
            .put("name", t.name)
            .put("duration_ms", t.durationMs)
            .put("artists", artists)
            .put(
                "album", org.json.JSONObject()
                    .put("id", t.albumId)
                    .put("name", t.albumName)
                    .put("images", images),
            )
            .put("uri", t.uri)
            .put("added_at", t.addedAt)
    }

    fun track(o: JSONObject): Track {
        val (names, ids) = namesAndIds(o.optJSONArray("artists"))
        return Track(
            id = o.optString("id", ""),
            name = o.optString("name", ""),
            artists = names,
            artistIds = ids,
            albumName = o.optJSONObject("album")?.optString("name", "") ?: "",
            albumId = o.optJSONObject("album")?.optString("id", "") ?: "",
            coverUrl = widest(o.optJSONObject("album")?.optJSONArray("images")),
            durationMs = o.optLong("duration_ms", o.optLong("durationMs", 0)),
            uri = o.optString("uri", ""),
            addedAt = o.optString("added_at", ""),
        )
    }

    fun album(o: JSONObject): Album {
        val (names, ids) = namesAndIds(o.optJSONArray("artists"))
        return Album(
            id = o.optString("id", ""),
            name = o.optString("name", ""),
            artists = names,
            artistIds = ids,
            coverUrl = widest(o.optJSONArray("images")),
            // Library counts are unreliable (often 0) — the detail page carries
            // the real count; shelves hide "0 tracks" like the current build.
            trackCount = o.optInt("total_tracks", o.optInt("track_count", 0)),
        )
    }

    fun artist(o: JSONObject): Artist = Artist(
        id = o.optString("id", ""),
        name = o.optString("name", ""),
        imageUrl = widest(o.optJSONArray("images")),
    )

    fun playlist(o: JSONObject): Playlist {
        val tracks = o.optJSONObject("tracks")
        return Playlist(
            id = o.optString("id", ""),
            name = o.optString("name", ""),
            coverUrl = widest(o.optJSONArray("images")),
            trackCount = tracks?.optInt("total", 0) ?: 0,
        )
    }

    /** Playlist detail tracks (TracksMeta.items are full Track objects). */
    fun playlistTracks(o: JSONObject): List<Track> =
        tracks(o.optJSONObject("tracks")?.optJSONArray("items"))

    fun tracks(a: JSONArray?): List<Track> {
        if (a == null) return emptyList()
        return List(a.length()) { i -> a.optJSONObject(i) }
            .filterNotNull()
            .map(::track)
            .filter { it.playable }
    }

    /** SavedTrack page: items[].track, skipping null (region-blocked). */
    fun savedTracks(page: JSONObject): List<Track> {
        val items = page.optJSONArray("items") ?: return emptyList()
        return List(items.length()) { i ->
            items.optJSONObject(i)?.optJSONObject("track")
        }.filterNotNull().map(::track).filter { it.playable }
    }

    fun pageTotal(o: JSONObject, fallback: Int): Int =
        o.optInt("total", fallback)

    /** Parallel name+ID lists for `artists`, filtered as one row set so indices stay paired. */
    private fun namesAndIds(a: JSONArray?): Pair<List<String>, List<String>> {
        if (a == null) return emptyList<String>() to emptyList()
        val names = mutableListOf<String>()
        val ids = mutableListOf<String>()
        for (i in 0 until a.length()) {
            val item = a.opt(i) ?: continue
            val name = if (item is String) item else (item as? JSONObject)?.optString("name", "") ?: ""
            if (name.isEmpty()) continue
            names += name
            ids += (item as? JSONObject)?.optString("id", "") ?: ""
        }
        return names to ids
    }

    private fun widest(a: JSONArray?): String {
        if (a == null) return ""
        var best = ""
        var bestW = -1
        for (i in 0 until a.length()) {
            val o = a.optJSONObject(i) ?: continue
            val w = o.optInt("width", 0)
            val url = o.optString("url", "")
            if (url.isNotEmpty() && w >= bestW) {
                bestW = w
                best = url
            }
        }
        return best
    }
}
