package com.spotifydx.app

import android.os.Bundle
import android.view.LayoutInflater
import android.view.View
import android.view.ViewGroup
import androidx.fragment.app.Fragment
import androidx.lifecycle.lifecycleScope
import androidx.recyclerview.widget.LinearLayoutManager
import androidx.recyclerview.widget.RecyclerView
import com.google.android.material.bottomsheet.BottomSheetDialogFragment
import kotlinx.coroutines.launch

/**
 * Full playing-history sheet (opened from the Library header button):
 * every played track, most recent first. Tap plays; dots/hold open the
 * same context menu as everywhere else. No arguments — history is live
 * repository state, so the sheet always shows the latest on open.
 */
class HistorySheet : BottomSheetDialogFragment() {
    override fun onCreateView(i: LayoutInflater, c: ViewGroup?, s: Bundle?): View =
        i.inflate(R.layout.dialog_history, c, false)

    override fun onViewCreated(v: View, s: Bundle?) {
        val list: RecyclerView = v.findViewById(R.id.history_list)
        list.layoutManager = LinearLayoutManager(context)
        val host: Fragment = this
        val adapter = TrackAdapter(
            onPlay = { PlayerRepository.play(it, "History") },
            onMenu = { ContextMenuHost.showMenu(host.parentFragmentManager, MenuTarget.Song(it)) },
        )
        list.adapter = adapter
        list.swipeToQueue(adapter)
        viewLifecycleOwner.lifecycleScope.launch {
            PlayerRepository.state.collect { st ->
                adapter.submitList(st.history.reversed())
            }
        }
    }
}
