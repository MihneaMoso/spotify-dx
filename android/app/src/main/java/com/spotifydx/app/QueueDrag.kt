package com.spotifydx.app

import androidx.recyclerview.widget.ItemTouchHelper
import androidx.recyclerview.widget.RecyclerView

/**
 * Drag-to-reorder for the unified queue timeline, Echo Music's queue
 * (`ui/player/Queue.kt`) ported to Views:
 *
 * - Drags start INSTANTLY from the row's drag handle (Echo's
 *   `draggableHandle()` — no long-press wait). Long-press drag is off.
 * - The ADAPTER owns its windows as one mutable list (Echo's
 *   `mutableQueueWindows`): [QueueTimelineAdapter.swapWindows] swaps data
 *   and notifies in the same main-thread call, so positions are exact at
 *   every instant — no differ, no parallel kind arrays, no deferred
 *   submits, nothing to desync or snap back.
 * - On drop ([clearView]) the gesture resolves once via
 *   [PlayerRepository.commitDrop] — Echo's commit-on-drop
 *   (`moveMediaItem` once, never mid-drag). The commit anchors to
 *   surviving rows in live repo state, so a track ending mid-drag (or any
 *   other queue mutation) can neither corrupt the order nor silently
 *   discard the gesture: worst case it abandons into a resync. The
 *   current track object is never touched — Echo's rapid song-switching
 *   on cross-current moves cannot happen here (reorder never calls
 *   play/seek — it only re-files tracks around a stationary current).
 *
 * Coexists with swipe-remove ([swipeToRemove], separate helper).
 */
class QueueDrag(
    private val adapter: QueueTimelineAdapter,
) : ItemTouchHelper.SimpleCallback(
    ItemTouchHelper.UP or ItemTouchHelper.DOWN,
    0,
) {
    override fun isLongPressDragEnabled(): Boolean = false

    /** Visual lift index at drag start; drop anchor at release. */
    private var liftPos: Int = RecyclerView.NO_POSITION
    private var hoverPos: Int = RecyclerView.NO_POSITION

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
        if (liftPos == RecyclerView.NO_POSITION) liftPos = from
        hoverPos = to
        adapter.beginDrag()
        return adapter.swapWindows(from, to)
    }

    override fun onSwiped(holder: RecyclerView.ViewHolder, direction: Int) {}

    override fun onSelectedChanged(
        holder: RecyclerView.ViewHolder?,
        actionState: Int,
    ) {
        super.onSelectedChanged(holder, actionState)
        if (actionState == ItemTouchHelper.ACTION_STATE_DRAG) {
            liftPos = holder?.bindingAdapterPosition ?: RecyclerView.NO_POSITION
            hoverPos = liftPos
            adapter.beginDrag()
            holder?.itemView?.alpha = 0.7f
        }
    }

    override fun clearView(rv: RecyclerView, holder: RecyclerView.ViewHolder) {
        super.clearView(rv, holder)
        holder.itemView.alpha = 1f
        val session = adapter.endDrag()
        val lift = liftPos
        val hover = hoverPos
        liftPos = RecyclerView.NO_POSITION
        hoverPos = RecyclerView.NO_POSITION
        if (lift == RecyclerView.NO_POSITION) return
        if (!PlayerRepository.commitDrop(session, lift, hover)) {
            // Content changed beyond anchor repair (rare): resync to live
            // truth instead of persisting anything stale.
            adapter.forceResync(PlayerRepository.timeline())
        }
    }
}

/**
 * Attach drag-reorder to a queue timeline list and arm its rows' drag
 * handles: touching a handle starts the drag instantly on that holder.
 */
fun RecyclerView.queueDrag(adapter: QueueTimelineAdapter) {
    val helper = ItemTouchHelper(QueueDrag(adapter))
    adapter.onHandleTouch = { holder -> helper.startDrag(holder) }
    helper.attachToRecyclerView(this)
}
