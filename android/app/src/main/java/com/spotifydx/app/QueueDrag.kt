package com.spotifydx.app

import androidx.recyclerview.widget.ItemTouchHelper
import androidx.recyclerview.widget.RecyclerView

/**
 * Drag-to-reorder for queue lists (any-music-app semantics): long-press a
 * row to lift it (dimmed while held), drop to commit. Commits through
 * [PlayerRepository.moveQueue], which reorders state AND persists (the
 * existing debounced replace) — order survives restarts.
 *
 * Coexists with [SwipeToQueue] on the same list (separate helpers: drag
 * uses long-press, swipe uses fling). The manual `notifyItemMoved` gives
 * live drag feedback; the StateFlow `submitList` reconciles after.
 */
class QueueDrag(
    private val adapter: TrackAdapter,
) : ItemTouchHelper.SimpleCallback(
    ItemTouchHelper.UP or ItemTouchHelper.DOWN,
    0,
) {
    override fun onMove(
        rv: RecyclerView,
        holder: RecyclerView.ViewHolder,
        target: RecyclerView.ViewHolder,
    ): Boolean {
        val from = holder.bindingAdapterPosition
        val to = target.bindingAdapterPosition
        if (from == RecyclerView.NO_POSITION || to == RecyclerView.NO_POSITION) {
            return false
        }
        PlayerRepository.moveQueue(from, to)
        adapter.notifyItemMoved(from, to)
        return true
    }

    override fun onSwiped(holder: RecyclerView.ViewHolder, direction: Int) {}

    override fun onSelectedChanged(
        holder: RecyclerView.ViewHolder?,
        actionState: Int,
    ) {
        super.onSelectedChanged(holder, actionState)
        if (actionState == ItemTouchHelper.ACTION_STATE_DRAG) {
            holder?.itemView?.alpha = 0.7f
        }
    }

    override fun clearView(rv: RecyclerView, holder: RecyclerView.ViewHolder) {
        super.clearView(rv, holder)
        holder.itemView.alpha = 1f
    }
}

/** Attach drag-reorder to a queue list. */
fun RecyclerView.queueDrag(adapter: TrackAdapter) {
    ItemTouchHelper(QueueDrag(adapter)).attachToRecyclerView(this)
}
