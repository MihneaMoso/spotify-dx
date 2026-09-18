package com.spotifydx.app

import android.app.DownloadManager
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Environment
import android.webkit.URLUtil
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch

/**
 * Context-menu action bodies (one per grid card). Async work launches in a
 * caller-supplied scope that outlives the sheet (the calling fragment's
 * scope, or the activity's when the sheet has no parent fragment — e.g.
 * the player-sheet queue) — dismissing the sheet never cancels Play or
 * Download.
 *
 * Stubs (`saveToPlaylist`, `saveToLibrary`, `like`) intentionally stop at
 * a toast: they need server-side writes (GQL POST mutations the core
 * fetcher doesn't support yet). The TODO(gql-POST) markers are the
 * plug-in seams, not dead ends.
 */
object ContextMenuActions {
    /** Play: song plays now; collections play first + insert rest at head. */
    fun play(scope: CoroutineScope, target: MenuTarget) {
        scope.launch {
            val tracks = target.contextTracks().getOrElse {
                ToastBus.fromBridge(it)
                return@launch
            }.filter { it.playable }
            if (tracks.isEmpty()) {
                ToastBus.error("Nothing playable")
                return@launch
            }
            val first = tracks.first()
            val rest = tracks.drop(1)
            if (target is MenuTarget.Song || rest.isEmpty()) {
                PlayerRepository.play(first, "Menu")
            } else {
                PlayerRepository.playContext(first, rest, "Menu")
            }
        }
    }

    /** Add to next: collection tracks land at the upcoming head, in order. */
    fun addNext(scope: CoroutineScope, target: MenuTarget) {
        scope.launch {
            val tracks = target.contextTracks().getOrElse {
                ToastBus.fromBridge(it)
                return@launch
            }.filter { it.playable }
            if (tracks.isEmpty()) {
                ToastBus.error("Nothing to queue")
                return@launch
            }
            PlayerRepository.playNext(tracks)
            ToastBus.error(
                if (tracks.size == 1) "Will play next" else "${tracks.size} tracks will play next",
            )
        }
    }

    /** Add to queue: append (single) or append-all, deduped by id. */
    fun enqueue(scope: CoroutineScope, target: MenuTarget) {
        scope.launch {
            val tracks = target.contextTracks().getOrElse {
                ToastBus.fromBridge(it)
                return@launch
            }.filter { it.playable }
            if (tracks.isEmpty()) {
                ToastBus.error("Nothing to queue")
                return@launch
            }
            tracks.forEach { PlayerRepository.enqueue(it) }
            ToastBus.error(
                if (tracks.size == 1) "Added to queue" else "${tracks.size} tracks added to queue",
            )
        }
    }

    // TODO(gql-POST): needs a core playlist-mutation (reads-only today).
    fun saveToPlaylist() {
        ToastBus.error("Not implemented yet")
    }

    /**
     * Download into the user's Downloads folder (one system notification
     * per track). The resolver already picks the highest-quality stream
     * the open engine can serve — no quality choice to make here.
     * DownloadManager owns the transfer (retries, completion UI); no
     * storage permission needed on modern Android for the shared
     * Downloads collection.
     */
    fun download(scope: CoroutineScope, ctx: Context, target: MenuTarget) {
        scope.launch {
            val tracks = target.contextTracks().getOrElse {
                ToastBus.fromBridge(it)
                return@launch
            }.filter { it.playable }
            if (tracks.isEmpty()) {
                ToastBus.error("Nothing to download")
                return@launch
            }
            val dm = ctx.getSystemService(DownloadManager::class.java) ?: run {
                ToastBus.error("Downloads unavailable")
                return@launch
            }
            var ok = 0
            var failed = 0
            tracks.forEach { t ->
                val url = MusicRepository.resolveStream(t).getOrNull()
                    ?.optString("url", "").orEmpty()
                if (url.isEmpty()) {
                    failed += 1
                    return@forEach
                }
                val fileName = URLUtil.guessFileName(url, null, null)
                    .takeIf { it.isNotBlank() }
                    ?: "${t.id}.mp3"
                try {
                    dm.enqueue(
                        DownloadManager.Request(Uri.parse(url))
                            .setTitle(
                                "${t.artistNames} — ${t.name}".takeIf { it.length > 2 } ?: t.name,
                            )
                            .setDescription("Spotify DX")
                            .setNotificationVisibility(
                                DownloadManager.Request.VISIBILITY_VISIBLE_NOTIFY_COMPLETED,
                            )
                            .setDestinationInExternalPublicDir(
                                Environment.DIRECTORY_DOWNLOADS,
                                "SpotifyDX/$fileName",
                            )
                            .setAllowedOverMetered(true),
                    )
                    ok += 1
                } catch (e: Exception) {
                    failed += 1
                }
            }
            ToastBus.error(
                when {
                    ok > 0 && failed == 0 ->
                        if (ok == 1) "Downloading to Downloads/SpotifyDX" else "Downloading $ok tracks"
                    ok > 0 -> "Downloading $ok tracks ($failed failed)"
                    else -> "Download failed"
                },
            )
        }
    }

    // TODO(gql-POST): needs a core library-mutation (reads-only today).
    fun saveToLibrary() {
        ToastBus.error("Not implemented yet")
    }

    // TODO(gql-POST): needs a core like-mutation (reads-only today).
    fun like() {
        ToastBus.error("Not implemented yet")
    }

    /** Native share sheet with the open.spotify.com link. */
    fun share(ctx: Context, target: MenuTarget) {
        val send = Intent(Intent.ACTION_SEND)
            .setType("text/plain")
            .putExtra(Intent.EXTRA_TEXT, target.shareUrl())
        ctx.startActivity(Intent.createChooser(send, "Share"))
    }
}
