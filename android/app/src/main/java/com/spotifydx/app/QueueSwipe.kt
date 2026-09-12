package com.spotifydx.app

import androidx.recyclerview.widget.ItemTouchHelper
import androidx.recyclerview.widget.RecyclerView

/**
 * Swipe-to-remove for the unified queue timeline (Echo's dismiss-to-remove
 * with Snackbar Undo — `rememberSwipeToDismissBoxState` + snackbar in
 * `ui/player/Queue.kt`). Either direction dismisses; the NOW row is
 * exempt (like Echo's locked/current rows). Removal is instant in the
 * adapter; the caller persists via [PlayerRepository.deleteTimelineEntry]
 * and offers Undo re-inserting via [PlayerRepository.insertTimelineEntry].
 */
class QueueSwipe(
    private val adapter: QueueTimelineAdapter,
    private val onSwiped: (entry: QueueEntry, position: Int) -> Unit,
) : ItemTouchHelper.SimpleCallback(
    0,
    ItemTouchHelper.LEFT or ItemTouchHelper.RIGHT,
) {
    override fun onMove(
        rv: RecyclerView,
        holder: RecyclerView.ViewHolder,
        target: RecyclerView.ViewHolder,
    ): Boolean = false

    override fun getSwipeDirs(
        rv: RecyclerView,
        holder: RecyclerView.ViewHolder,
    ): Int {
        val pos = holder.bindingAdapterPosition
        if (pos == RecyclerView.NO_POSITION) return 0
        // NOW never dismisses (pinned anchor, same rule as dragging).
        if (adapter.kindAt(pos) == RowKind.NOW) return 0
        return super.getSwipeDirs(rv, holder)
    }

    override fun onSwiped(holder: RecyclerView.ViewHolder, direction: Int) {
        val pos = holder.bindingAdapterPosition
        if (pos == RecyclerView.NO_POSITION) return
        adapter.removeAt(pos)?.let { onSwiped(it, pos) }
    }
}

/** Attach Echo-style swipe-to-remove to a queue timeline list. */
fun RecyclerView.swipeToRemove(
    adapter: QueueTimelineAdapter,
    onSwiped: (entry: QueueEntry, position: Int) -> Unit,
) {
    ItemTouchHelper(QueueSwipe(adapter, onSwiped)).attachToRecyclerView(this)
}
