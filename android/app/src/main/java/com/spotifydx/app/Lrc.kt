package com.spotifydx.app

/**
 * LRC line parsing + lookup for synced lyrics (Phase 5). Line-level sync
 * only (word timing is rarely available): `[mm:ss.xx] text` → millisecond
 * timeline; metadata tags (`[ar:]`, `[ti:]`…) and untimestamped lines are
 * skipped. Pure Kotlin — no Android dependencies.
 */
data class LrcLine(val ms: Long, val text: String)

object Lrc {
    private val LINE = Regex("""^\[(\d+):(\d{2})(?:\.(\d{2,3}))?\](.*)$""")

    /** Parse synced text into timeline order. Empty when nothing parses. */
    fun parse(synced: String): List<LrcLine> =
        synced.lineSequence()
            .mapNotNull { raw ->
                val m = LINE.matchEntire(raw.trim()) ?: return@mapNotNull null
                val min = m.groupValues[1].toLongOrNull() ?: return@mapNotNull null
                val sec = m.groupValues[2].toLongOrNull() ?: return@mapNotNull null
                val frac = m.groupValues[3]
                val fracMs = when (frac.length) {
                    3 -> frac.toLongOrNull() ?: 0L
                    2 -> (frac.toLongOrNull() ?: 0L) * 10
                    else -> 0L
                }
                val text = m.groupValues[4].trim()
                if (text.isEmpty()) return@mapNotNull null
                LrcLine((min * 60 + sec) * 1000 + fracMs, text)
            }
            .sortedBy { it.ms }
            .toList()

    /** Index of the active line at [positionMs] (binary search, O(log n)). */
    fun indexAt(lines: List<LrcLine>, positionMs: Long): Int {
        var lo = 0
        var hi = lines.size - 1
        var idx = 0
        while (lo <= hi) {
            val mid = (lo + hi) ushr 1
            if (lines[mid].ms <= positionMs) {
                idx = mid
                lo = mid + 1
            } else {
                hi = mid - 1
            }
        }
        return idx
    }
}
