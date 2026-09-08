package com.spotifydx.app

import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.util.Base64
import android.util.LruCache
import android.widget.ImageView
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * Artwork pipeline (§9.6 ARCHITECTURE): deterministic seed-colored
 * placeholder, then the full image (blur-up preview is hardening work).
 * Two tiers: process memory here, the core's hash-keyed disk cache +
 * ad-filter gate behind `fetchArtwork` (empty URLs fail fast, never hit
 * the bridge).
 */
object ArtworkLoader {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val mem = object : LruCache<String, Bitmap>(64) {}

    fun load(view: ImageView, url: String) {
        if (url.isEmpty()) {
            view.setImageDrawable(null)
            view.setBackgroundColor(0xFF1A2136.toInt())
            return
        }
        mem.get(url)?.let {
            view.setImageBitmap(it)
            return
        }
        view.setImageDrawable(null)
        view.setBackgroundColor(seedColor(url))
        scope.launch {
            val bytes = withContext(Dispatchers.IO) {
                val b64 = MusicRepository.artwork(url).getOrNull() ?: return@withContext null
                runCatching { Base64.decode(b64, Base64.DEFAULT) }.getOrNull()
            } ?: return@launch
            val bmp = withContext(Dispatchers.Default) {
                BitmapFactory.decodeByteArray(bytes, 0, bytes.size)
            } ?: return@launch
            mem.put(url, bmp)
            // The view may be recycled; only bind if still expecting this URL.
            if (view.getTag(R.id.track_art) == url) {
                view.setImageBitmap(bmp)
            }
        }
    }

    private fun seedColor(url: String): Int {
        val h = url.hashCode()
        val r = 22 + (h and 0x1F)
        val g = 28 + ((h shr 5) and 0x1F)
        val b = 48 + ((h shr 10) and 0x1F)
        return (0xFF shl 24) or (r shl 16) or (g shl 8) or b
    }
}
