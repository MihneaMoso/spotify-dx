package com.spotifydx.app

import android.animation.ValueAnimator
import android.graphics.Canvas
import android.graphics.ColorFilter
import android.graphics.Paint
import android.graphics.PixelFormat
import android.graphics.drawable.Drawable
import android.view.HapticFeedbackConstants
import android.view.View
import kotlin.math.hypot

/**
 * Long-press-to-menu arming (Echo parity): the framework's own long-press
 * (~500ms, no custom timer, zero per-touch work) fires the menu. Any
 * scroll, swipe, or drag cancels it in the framework before it can fire,
 * so queue swipes, timeline drags, and plain taps never pay for it.
 *
 * The only animation is a single slow wave burst played *at fire time*
 * (concurrent with the sheet sliding up) — nothing renders, allocates,
 * or invalidates on ordinary taps or scrolls.
 */
object HoldToOpen {
    /** Fire-time wave length (slow and gentle, Echo feel). */
    private const val BURST_MS = 700L

    fun arm(view: View, onHold: () -> Unit) {
        // A previous arming may have left a touch listener behind.
        view.setOnTouchListener(null)
        view.setOnLongClickListener { v ->
            fireWave(v)
            v.performHapticFeedback(HapticFeedbackConstants.LONG_PRESS)
            onHold()
            true
        }
    }

    /**
     * One-shot expanding wave from the row's center, drawn in the view
     * overlay (zero layout) and removed on animation end. Created only on
     * an actual long-press — scrolls and taps never reach here.
     */
    private fun fireWave(view: View) {
        if (!view.isAttachedToWindow || view.width == 0 || view.height == 0) return
        val cx = view.width / 2f
        val cy = view.height / 2f
        val wave = WaveDrawable(cx, cy, hypot(view.width.toFloat(), view.height.toFloat()))
        wave.setBounds(0, 0, view.width, view.height)
        view.overlay.add(wave)
        ValueAnimator.ofFloat(0f, 1f).apply {
            duration = BURST_MS
            addUpdateListener {
                wave.progress = it.animatedValue as Float
                wave.invalidateSelf()
            }
            addListener(object : android.animation.AnimatorListenerAdapter() {
                override fun onAnimationEnd(a: android.animation.Animator) {
                    view.overlay.remove(wave)
                }

                override fun onAnimationCancel(a: android.animation.Animator) {
                    view.overlay.remove(wave)
                }
            })
        }.start()
    }

    private class WaveDrawable(
        private val cx: Float,
        private val cy: Float,
        private val maxR: Float,
    ) : Drawable() {
        var progress: Float = 0f

        private val ring = Paint(Paint.ANTI_ALIAS_FLAG).apply {
            style = Paint.Style.STROKE
            strokeWidth = 3f
            color = android.graphics.Color.WHITE
        }
        private val press = Paint(Paint.ANTI_ALIAS_FLAG).apply {
            style = Paint.Style.FILL
            color = android.graphics.Color.WHITE
        }

        override fun draw(canvas: Canvas) {
            press.alpha = 16
            canvas.drawCircle(cx, cy, 28f, press)
            for (i in 0..1) {
                val p = (progress * 2f - i * 0.5f).coerceIn(0f, 1f)
                if (p <= 0f) continue
                ring.alpha = ((1f - p) * 90).toInt()
                canvas.drawCircle(cx, cy, 28f + p * (maxR - 28f), ring)
            }
        }

        override fun setAlpha(alpha: Int) {}
        override fun setColorFilter(filter: ColorFilter?) {}
        override fun getOpacity(): Int = PixelFormat.TRANSLUCENT
    }
}
