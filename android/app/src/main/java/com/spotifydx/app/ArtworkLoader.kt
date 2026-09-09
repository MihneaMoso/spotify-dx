package com.spotifydx.app

import android.content.Context
import android.graphics.drawable.ColorDrawable
import android.util.Base64
import android.widget.ImageView
import coil.ImageLoader
import coil.disk.DiskCache
import coil.dispose
import coil.fetch.FetchResult
import coil.fetch.Fetcher
import coil.fetch.SourceResult
import coil.load
import coil.memory.MemoryCache
import coil.request.Options
import okio.Buffer

/**
 * Artwork pipeline on Coil 2 (memory + disk cache, request dedup and
 * recycle-safe rebinding handled by the library).
 *
 * Bytes flow through the core gate ([MusicRepository.artwork] → core disk
 * cache + ad-filter), never direct HTTP: [CoreGateFetcher] adapts the gate
 * to Coil's fetcher API. Coil's disk cache then keeps the decoded-source
 * bytes across restarts (no repeat JNI/base64), and its size-based memory
 * cache replaces the old count-based LruCache (bitmap OOM risk).
 */
object ArtworkLoader {
    @Volatile
    private var loader: ImageLoader? = null

    private fun loader(ctx: Context): ImageLoader =
        loader ?: synchronized(this) {
            loader ?: ImageLoader.Builder(ctx.applicationContext)
                .components { add(CoreGateFetcher.Factory()) }
                .memoryCache {
                    MemoryCache.Builder(ctx.applicationContext).maxSizePercent(0.25).build()
                }
                .diskCache {
                    DiskCache.Builder()
                        .directory(ctx.applicationContext.cacheDir.resolve("artwork"))
                        .maxSizeBytes(100L * 1024 * 1024)
                        .build()
                }
                // Gate responses carry no HTTP cache headers; cache by URL.
                .respectCacheHeaders(false)
                .build()
                .also { loader = it }
        }

    fun load(view: ImageView, url: String) {
        if (url.isEmpty()) {
            view.dispose()
            view.setImageDrawable(null)
            view.setBackgroundColor(0xFF1A2136.toInt())
            return
        }
        view.load(url, loader(view.context)) {
            placeholder(ColorDrawable(seedColor(url)))
            error(ColorDrawable(seedColor(url)))
            crossfade(true)
        }
    }

    /** Deterministic seed-colored placeholder (kept from the old pipeline). */
    private fun seedColor(url: String): Int {
        val h = url.hashCode()
        val r = 22 + (h and 0x1F)
        val g = 28 + ((h shr 5) and 0x1F)
        val b = 48 + ((h shr 10) and 0x1F)
        return (0xFF shl 24) or (r shl 16) or (g shl 8) or b
    }

    /**
     * Adapts the core artwork gate to Coil: base64 over JNI → bytes.
     * Coil owns threading (never the interface thread), retries, and both
     * cache tiers around this.
     */
    private class CoreGateFetcher(
        private val url: String,
        private val context: Context,
    ) : Fetcher {
        override suspend fun fetch(): FetchResult {
            val b64 = MusicRepository.artwork(url).getOrNull()
                ?: throw IllegalStateException("artwork unavailable: ${url.take(80)}")
            val bytes = try {
                Base64.decode(b64, Base64.DEFAULT)
            } catch (e: IllegalArgumentException) {
                throw IllegalStateException("artwork decode failed: ${e.message}")
            }
            if (bytes.isEmpty()) throw IllegalStateException("artwork empty")
            return SourceResult(
                source = coil.decode.ImageSource(
                    source = Buffer().write(bytes),
                    context = context,
                ),
                mimeType = null, // let Coil sniff (jpeg/png/webp)
                dataSource = coil.decode.DataSource.NETWORK,
            )
        }

        class Factory : Fetcher.Factory<String> {
            override fun create(
                data: String,
                options: Options,
                imageLoader: ImageLoader,
            ): Fetcher? =
                if (data.startsWith("http")) CoreGateFetcher(data, options.context) else null
        }
    }
}
