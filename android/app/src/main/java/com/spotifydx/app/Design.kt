package com.spotifydx.app

import android.content.Context
import android.graphics.Color
import androidx.annotation.AttrRes
import androidx.annotation.FloatRange

/**
 * Echo-port design helpers (Phase 1). Central home for the alpha-layer math
 * Echo applies pervasively (surfaceVariant 0.3/0.5, primary 0.4, scrims
 * 0.2-0.6) so later phases never hardcode translucent hexes in Kotlin.
 * Theme roles themselves keep resolving via ?attr/ in XML; this object is
 * only for runtime-computed colors (canvas/placeholder/Coil transforms).
 */
object Design {
    /** Alpha ratios mirroring Echo's Dimensions/layer conventions. */
    const val ALPHA_SELECTED = 0.4f
    const val ALPHA_PLAYING = 0.2f
    const val ALPHA_SHELF = 0.3f
    const val ALPHA_FIELD = 0.5f
    const val ALPHA_SCRIM = 0.6f

    /** Returns [color] with its alpha multiplied by [ratio]. */
    fun withAlpha(color: Int, @FloatRange(from = 0.0, to = 1.0) ratio: Float): Int {
        val a = (Color.alpha(color) * ratio).toInt().coerceIn(0, 255)
        return Color.argb(a, Color.red(color), Color.green(color), Color.blue(color))
    }

    /** Resolves a theme color attr (e.g. android.R.attr.colorBackground) to ARGB. */
    fun resolveAttr(context: Context, @AttrRes attr: Int): Int {
        val tv = android.util.TypedValue()
        context.theme.resolveAttribute(attr, tv, true)
        return tv.data
    }

    /** Resolves a theme color attr and applies [ratio] alpha (Echo layer math). */
    fun layer(
        context: Context,
        @AttrRes attr: Int,
        @FloatRange(from = 0.0, to = 1.0) ratio: Float,
    ): Int = withAlpha(resolveAttr(context, attr), ratio)

    /**
     * Clips a view (typically an ImageView) to a rounded rect of [radiusPx].
     * Echo rounds artwork in code-composed clips (6dp list / 12dp player);
     * on Views the equivalent is an outline clip — no layout change needed,
     * and it rounds placeholders, crossfades and bitmaps alike. Idempotent:
     * safe to call on every bind (recycled rows keep their own radius).
     */
    fun clipRounded(view: android.view.View, radiusPx: Float) {
        view.clipToOutline = true
        view.outlineProvider = object : android.view.ViewOutlineProvider() {
            override fun getOutline(v: android.view.View, outline: android.graphics.Outline) {
                outline.setRoundRect(0, 0, v.width, v.height, radiusPx)
            }
        }
        // Outline is measured at layout time; re-clip once laid out so
        // recycled views with stale bounds round correctly.
        view.post { view.invalidateOutline() }
    }

    /** Circular variant (Echo artist/avatar treatment). */
    fun clipCircle(view: android.view.View) {
        view.clipToOutline = true
        view.outlineProvider = object : android.view.ViewOutlineProvider() {
            override fun getOutline(v: android.view.View, outline: android.graphics.Outline) {
                outline.setOval(0, 0, v.width, v.height)
            }
        }
        view.post { view.invalidateOutline() }
    }
}
