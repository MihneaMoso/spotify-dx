package com.spotifydx.app

import android.content.Context
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.RectF
import android.view.View
import androidx.appcompat.content.res.AppCompatResources
import androidx.core.content.ContextCompat
import androidx.core.graphics.drawable.toBitmap
import androidx.recyclerview.widget.ItemTouchHelper
import androidx.recyclerview.widget.RecyclerView
import kotlin.math.abs
import kotlin.math.min

/**
 * Spotify-style swipe-to-queue for track rows: sliding a row left or right
 * reveals the green queue affordance behind it (icon alpha ramps up to the
 * commit threshold); on release past threshold the track enqueues and the
 * row springs back. Cancel (short swipe) animates back with no side effect.
 *
 * Uses platform [ItemTouchHelper] — no hand-rolled touch handling. Attach to
 * every track list via [RecyclerView.swipeToQueue].
 */
class SwipeToQueue(
    private val adapter: TrackAdapter,
    private val onQueue: (Track) -> Unit,
) : ItemTouchHelper.SimpleCallback(0, ItemTouchHelper.LEFT or ItemTouchHelper.RIGHT) {

    private var bgPaint: Paint? = null
    private var icon: Bitmap? = null
    private var iconSizePx: Int = 0
    private var cornerPx: Float = 0f

    override fun onMove(
        rv: RecyclerView,
        holder: RecyclerView.ViewHolder,
        target: RecyclerView.ViewHolder,
    ): Boolean = false

    /** Commit slightly before halfway — snappier than the default 0.5. */
    override fun getSwipeThreshold(holder: RecyclerView.ViewHolder): Float = 0.4f

    override fun onSwiped(holder: RecyclerView.ViewHolder, direction: Int) {
        val pos = holder.bindingAdapterPosition
        if (pos != RecyclerView.NO_POSITION) {
            adapter.currentList.getOrNull(pos)?.let(onQueue)
            // Dataset unchanged: rebind to spring the row back into place.
            adapter.notifyItemChanged(pos)
        }
    }

    override fun onChildDraw(
        c: Canvas,
        rv: RecyclerView,
        holder: RecyclerView.ViewHolder,
        dX: Float,
        dY: Float,
        actionState: Int,
        isActive: Boolean,
    ) {
        if (actionState == ItemTouchHelper.ACTION_STATE_SWIPE) {
            drawReveal(c, rv.context, holder.itemView, dX)
        }
        super.onChildDraw(c, rv, holder, dX, dY, actionState, isActive)
    }

    private fun drawReveal(c: Canvas, ctx: Context, item: View, dX: Float) {
        if (dX == 0f) return
        val paint = bgPaint ?: Paint(Paint.ANTI_ALIAS_FLAG).apply {
            color = ContextCompat.getColor(ctx, R.color.spotify_green)
            bgPaint = this
        }
        if (iconSizePx == 0) {
            val density = ctx.resources.displayMetrics.density
            iconSizePx = (48 * density).toInt()
            cornerPx = 12 * density
        }
        val bmp = icon ?: AppCompatResources.getDrawable(ctx, R.drawable.ic_queue_add)
            ?.toBitmap(iconSizePx, iconSizePx)?.also { icon = it }
            ?: return

        val w = item.width.toFloat()
        val revealed = abs(dX)
        // Background fills the revealed side edge-to-edge behind the row.
        val bg = if (dX > 0) {
            RectF(item.left.toFloat(), item.top.toFloat(), item.left + revealed, item.bottom.toFloat())
        } else {
            RectF(item.right - revealed, item.top.toFloat(), item.right.toFloat(), item.bottom.toFloat())
        }
        c.drawRoundRect(bg, cornerPx, cornerPx, paint)

        // Icon rides centered in the revealed zone; alpha ramps to full at
        // the commit threshold (0.4 × width) so release intent is visible.
        paint.alpha = (255 * min(1f, revealed / (w * 0.4f))).toInt()
        val cx = if (dX > 0) bg.left + revealed / 2 else bg.right - revealed / 2
        val cy = (item.top + item.bottom) / 2f
        val half = iconSizePx / 2f
        c.drawBitmap(bmp, cx - half, cy - half, paint)
        paint.alpha = 255
    }
}

/** Shared enqueue action (toast confirms, like the old long-press did). */
private fun queueWithToast(t: Track) {
    PlayerRepository.enqueue(t)
    ToastBus.error("Added to queue")
}

/** Attach Spotify-style swipe-to-queue to a track list. */
fun RecyclerView.swipeToQueue(adapter: TrackAdapter) {
    ItemTouchHelper(SwipeToQueue(adapter, ::queueWithToast)).attachToRecyclerView(this)
}
