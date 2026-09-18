package com.spotifydx.app

import org.json.JSONObject

/**
 * Context-menu model (Echo `MediaMoreBottomSheet` parity): every song,
 * album, artist, and playlist in the app funnels through one target type
 * and one sheet ([ContextMenuSheet]), whether triggered by the 2s hold
 * ([HoldToOpen]) or a dots button. Echo does exactly this single-funnel
 * split (`onMediaLongClicked` ← long-press + dots + toolbar overflow).
 */
sealed interface MenuTarget {
    data class Song(val track: Track) : MenuTarget
    data class Album(val album: com.spotifydx.app.Album) : MenuTarget
    data class Artist(val artist: com.spotifydx.app.Artist) : MenuTarget
    data class Playlist(val playlist: com.spotifydx.app.Playlist) : MenuTarget
}

/** Header content for the sheet: artwork, title line, subtitle line. */
fun MenuTarget.header(): Triple<String, String, String> = when (this) {
    is MenuTarget.Song -> Triple(track.coverUrl, track.name, track.artistNames)
    is MenuTarget.Album -> Triple(album.coverUrl, album.name, album.artists.joinToString(", "))
    is MenuTarget.Artist -> Triple(artist.imageUrl, artist.name, "Artist")
    is MenuTarget.Playlist -> Triple(
        playlist.coverUrl,
        playlist.name,
        if (playlist.trackCount > 0) "Playlist · ${playlist.trackCount} tracks" else "Playlist",
    )
}

/** Native-share link for the target (Spotify IDs are URL-safe as-is). */
fun MenuTarget.shareUrl(): String = when (this) {
    is MenuTarget.Song -> "https://open.spotify.com/track/${track.id}"
    is MenuTarget.Album -> "https://open.spotify.com/album/${album.id}"
    is MenuTarget.Artist -> "https://open.spotify.com/artist/${artist.id}"
    is MenuTarget.Playlist -> "https://open.spotify.com/playlist/${playlist.id}"
}

/** Artist nav entries (name + id), skipping ID-less entries (never dead buttons). */
fun MenuTarget.artistNav(): List<Pair<String, String>> = when (this) {
    is MenuTarget.Song -> track.artists.zip(track.artistIds)
        .filter { (name, id) -> name.isNotEmpty() && id.isNotEmpty() }
        .map { (name, id) -> name to id }
    is MenuTarget.Album -> album.artists.zip(album.artistIds)
        .filter { (name, id) -> name.isNotEmpty() && id.isNotEmpty() }
        .map { (name, id) -> name to id }
    else -> emptyList()
}

/** Album nav entry (name + id), or null when unknown. */
fun MenuTarget.albumNav(): Pair<String, String>? = when (this) {
    is MenuTarget.Song ->
        if (track.albumId.isNotEmpty() && track.albumName.isNotEmpty()) {
            track.albumName to track.albumId
        } else {
            null
        }
    else -> null
}

/**
 * Full track list behind a collection target (album/playlist play, batch
 * queue/download). Shapes mirror DetailViewModel.load — same bridge
 * payloads, same parsers, no second contract.
 */
suspend fun MenuTarget.contextTracks(): Result<List<Track>> {
    if (this is MenuTarget.Song) return Result.success(listOf(track))
    val id = when (this) {
        is MenuTarget.Album -> album.id
        is MenuTarget.Artist -> artist.id
        is MenuTarget.Playlist -> playlist.id
        else -> ""
    }
    if (id.isEmpty()) return Result.success(emptyList())
    return MusicRepository.withSessionCheck {
        when (this) {
            is MenuTarget.Album -> MusicRepository.album(id).map { json ->
                Models.tracks(json.optJSONArray("tracks"))
            }
            is MenuTarget.Artist -> MusicRepository.artistPage(id).map { json ->
                Models.tracks(json.optJSONArray("top_tracks"))
            }
            is MenuTarget.Playlist -> MusicRepository.playlist(id).map { json ->
                Models.playlistTracks(json)
            }
            else -> Result.success(emptyList())
        }
    }
}

/** Single funnel (Echo `onMediaLongClicked`): every trigger lands here. */
object ContextMenuHost {
    /**
     * Show the menu from a fragment: pass the fragment's
     * [androidx.fragment.app.Fragment.parentFragmentManager] so the sheet's
     * parentFragment becomes the caller — async actions then run in the
     * caller's scope and survive sheet dismissal.
     */
    fun showMenu(fm: androidx.fragment.app.FragmentManager, target: MenuTarget) {
        ContextMenuSheet.newInstance(target).show(fm, "context_menu")
    }
}

/** Sheet-argument round-trip for [MenuTarget] (dialog recreation-safe). */
fun MenuTarget.toArg(): JSONObject = when (this) {
    is MenuTarget.Song -> JSONObject()
        .put("type", "song")
        .put("payload", Models.trackToJson(track))
    is MenuTarget.Album -> {
        val artists = org.json.JSONArray()
        album.artists.forEachIndexed { i, name ->
            artists.put(JSONObject().put("name", name).put("id", album.artistIds.getOrNull(i) ?: ""))
        }
        JSONObject()
            .put("type", "album")
            .put(
                "payload", JSONObject()
                    .put("id", album.id)
                    .put("name", album.name)
                    .put("artists", artists)
                    .put("coverUrl", album.coverUrl),
            )
    }
    is MenuTarget.Artist -> JSONObject()
        .put("type", "artist")
        .put(
            "payload", JSONObject()
                .put("id", artist.id)
                .put("name", artist.name)
                .put("imageUrl", artist.imageUrl),
        )
    is MenuTarget.Playlist -> JSONObject()
        .put("type", "playlist")
        .put(
            "payload", JSONObject()
                .put("id", playlist.id)
                .put("name", playlist.name)
                .put("coverUrl", playlist.coverUrl)
                .put("trackCount", playlist.trackCount),
        )
}

fun menuTargetFromArg(o: JSONObject): MenuTarget? {
    val p = o.optJSONObject("payload") ?: return null
    return when (o.optString("type", "")) {
        "song" -> MenuTarget.Song(Models.track(p))
        "album" -> {
            val (names, ids) = Models.namesAndIds(p.optJSONArray("artists"))
            MenuTarget.Album(
                com.spotifydx.app.Album(
                    id = p.optString("id", ""),
                    name = p.optString("name", ""),
                    artists = names,
                    artistIds = ids,
                    coverUrl = p.optString("coverUrl", ""),
                ),
            )
        }
        "artist" -> MenuTarget.Artist(
            com.spotifydx.app.Artist(
                id = p.optString("id", ""),
                name = p.optString("name", ""),
                imageUrl = p.optString("imageUrl", ""),
            ),
        )
        "playlist" -> MenuTarget.Playlist(
            com.spotifydx.app.Playlist(
                id = p.optString("id", ""),
                name = p.optString("name", ""),
                coverUrl = p.optString("coverUrl", ""),
                trackCount = p.optInt("trackCount", 0),
            ),
        )
        else -> null
    }
}
