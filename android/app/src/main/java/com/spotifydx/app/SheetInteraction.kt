package com.spotifydx.app

/**
 * Pure interaction math for the sheet/mini-player drag system, the lyrics
 * preview window, the history highlight rebind, and the floating-chrome
 * content inset. Zero Android dependencies (px/floats/ids only) so the
 * JVM regression suite guards the exact numbers the touch listeners use.
 *
 * Call sites MUST delegate here — never duplicate these constants, or the
 * suite guards fiction while the app drifts.
 */
object SheetInteraction {
    // -- Mini-player bar drag (1:1 upward follow, quarter release) --------

    /** Upward-only 1:1 follow, clamped to the screen. Downward motion is
     * ignored (the bar never sinks below rest). */
    fun barFollow(translation: Float, dy: Float, screenH: Float): Float =
        (translation + dy.coerceAtMost(0f)).coerceIn(-screenH, 0f)

    /** Release hands off to the sheet iff the finger traveled up past a
     * quarter of the screen. */
    fun barReleaseOpens(rawY: Float, screenH: Float): Boolean =
        rawY < screenH * 3f / 4f

    // -- Sheet drag (1.5x amplified follow, quarter release) --------------

    /** Amplified downward follow (a flick covers real distance); upward
     * bleeds back toward rest, never above it. */
    fun sheetFollow(acc: Float, dy: Float): Float =
        (acc + dy * 1.5f).coerceAtLeast(0f)

    /** Release dismisses iff dragged down past a quarter of the screen. */
    fun sheetReleaseDismisses(acc: Float, fullH: Float): Boolean =
        acc > fullH / 4f

    // -- Lyrics preview window (prev2..next2) ------------------------------

    /** Five rows around [idx]; out-of-range edges are "" (row minHeights
     * keep the geometry stable while sliding). */
    fun previewWindow(lines: List<String>, idx: Int): List<String> =
        (-2..2).map { lines.getOrNull(idx + it) ?: "" }

    /** Preview box height ≈ 1/5 of the screen, clamped so small screens
     * keep the transport reachable and large screens don't bloat. */
    fun previewBoxHeightDp(screenHpx: Float, density: Float): Int =
        (screenHpx / 5f / density).toInt().coerceIn(112, 160)

    // -- History highlight rebind (DiffUtil moves never rebind) -----------

    /** Positions to rebind on a head change: the old head's new slot
     * (skipped when trimmed away — its holder recycled through bind
     * already), then the new head at 0. */
    fun highlightRebinds(ids: List<String>, oldHead: String?): List<Int> {
        val out = mutableListOf<Int>()
        if (oldHead != null) {
            val oldPos = ids.indexOfFirst { it == oldHead }
            if (oldPos >= 0) out.add(oldPos)
        }
        out.add(0)
        return out
    }

    // -- Floating-chrome content inset ------------------------------------

    /** Bottom inset matching the floating chrome stack (visible bar +
     * bottom nav; the rail variant has no bottom nav). */
    fun contentInset(barVisible: Boolean, barH: Int, navH: Int): Int =
        (if (barVisible) barH else 0) + navH

    /** The queue size describes the queue view: header count only there. */
    fun queueCountVisible(isQueueTab: Boolean): Boolean = isQueueTab
}
