package com.spotifydx.app

/**
 * Pure timeline math for the unified queue ([PlayerRepository.commitDrop]
 * anchoring). Operating on plain lists only — no repository state, no
 * persistence, no coroutines — so anchor reconciliation is testable and
 * reviewable in isolation. The repository keeps the state-touching
 * commits; everything here is a deterministic function of its inputs.
 */

/** 1-based occurrence rank of the id at pos within list. */
internal fun occurrenceRank(list: List<QueueEntry>, pos: Int, id: String): Int {
    if (pos !in list.indices) return 0
    return list.subList(0, pos + 1).count { it.track.id == id }
}

/** Index of the rank-th occurrence of id, or -1. */
internal fun findOccurrence(list: List<QueueEntry>, id: String, rank: Int): Int {
    if (rank <= 0) return -1
    var c = 0
    list.forEachIndexed { i, e ->
        if (e.track.id == id) {
            c++
            if (c == rank) return i
        }
    }
    return -1
}

internal fun sameIdMultiset(a: List<QueueEntry>, b: List<QueueEntry>): Boolean {
    if (a.size != b.size) return false
    return a.map { it.track.id }.sorted() == b.map { it.track.id }.sorted()
}
