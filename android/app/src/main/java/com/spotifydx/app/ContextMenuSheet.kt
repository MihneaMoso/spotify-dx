package com.spotifydx.app

import android.os.Bundle
import android.view.LayoutInflater
import android.view.View
import android.view.ViewGroup
import android.widget.ImageButton
import android.widget.ImageView
import android.widget.TextView
import androidx.lifecycle.lifecycleScope
import androidx.recyclerview.widget.GridLayoutManager
import androidx.recyclerview.widget.RecyclerView
import com.google.android.material.bottomsheet.BottomSheetBehavior
import com.google.android.material.bottomsheet.BottomSheetDialog
import com.google.android.material.bottomsheet.BottomSheetDialogFragment
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch

/**
 * Echo-style context menu (`MediaMoreBottomSheet` parity): ¾-screen sheet,
 * ✕ top-right, target header, 2-column action grid, then full-width
 * artist/album nav rows. Triggered by the 2s hold or any dots button —
 * both funnel through [ContextMenuHost] (Echo's `onMediaLongClicked`).
 *
 * Async actions run in the *calling* fragment's scope (via
 * [ContextMenuActions]), so dismissing the sheet never cancels Play or
 * Download. Material3 theming comes from `Theme.SpotifyDx`, so both
 * palettes style the sheet with no overlay needed.
 */
class ContextMenuSheet : BottomSheetDialogFragment() {
    companion object {
        private const val ARG_TARGET = "target"

        fun newInstance(target: MenuTarget): ContextMenuSheet =
            ContextMenuSheet().apply {
                arguments = Bundle().apply { putString(ARG_TARGET, target.toArg().toString()) }
            }
    }

    private var target: MenuTarget? = null

    override fun onCreate(s: Bundle?) {
        super.onCreate(s)
        target = runCatching {
            menuTargetFromArg(org.json.JSONObject(requireArguments().getString(ARG_TARGET, "{}")))
        }.getOrNull()
    }

    override fun onCreateView(i: LayoutInflater, c: ViewGroup?, s: Bundle?): View =
        i.inflate(R.layout.dialog_context_menu, c, false)

    override fun onViewCreated(v: View, s: Bundle?) {
        val t = target ?: run { dismissAllowingStateLoss(); return }
        val grid: RecyclerView = v.findViewById(R.id.menu_grid)
        grid.layoutManager = GridLayoutManager(context, 2).apply {
            spanSizeLookup = object : GridLayoutManager.SpanSizeLookup() {
                override fun getSpanSize(pos: Int): Int =
                    if ((grid.adapter as? Adapter)?.isAction(pos) == true) 1 else 2
            }
        }
        grid.adapter = Adapter(t, ::dismissAllowingStateLoss)
    }

    override fun onStart() {
        super.onStart()
        // ¾-screen peek, collapsed at open (Echo slide-up); content taller
        // than the peek scrolls inside, drag reaches full expansion.
        val d = dialog as? BottomSheetDialog ?: return
        val peek = (resources.displayMetrics.heightPixels * 0.75).toInt()
        // Bounded sheet contract (all three must hold together):
        // - fitToContents: expansion stops at the content height — a drag
        //   that hits the list end cannot push the sheet further up past
        //   what it should cover; over-tall content caps at full screen.
        // - peek + container minHeight: short menus still fill the same ¾
        //   boundary instead of wrapping into a stub with app showing below.
        // - non-hideable: no downward escape into HIDDEN past the peek.
        // Dragging itself stays fully enabled in both directions within
        // those bounds, so long menus expand to full and scroll inside.
        d.behavior.isFitToContents = true
        d.behavior.peekHeight = peek
        d.behavior.isHideable = false
        d.behavior.state = BottomSheetBehavior.STATE_COLLAPSED
        // Short menus (few actions, no nav rows) would otherwise wrap their
        // content and leave the app's bottom nav / mini player visible
        // below the sheet, with the sheet scrollable past its own edge.
        // Pin the sheet container to the peek height so the boundary is
        // identical for short and long menus: short content top-aligns and
        // doesn't scroll at all; long content scrolls inside up to full
        // expansion.
        val sheetView = d.findViewById<View>(com.google.android.material.R.id.design_bottom_sheet)
        sheetView?.minimumHeight = peek
        // Explicit opaque background + scrim: the app theme carries no
        // bottomSheetDialogTheme, and on-device the style fallback left
        // the sheet transparent AND the backdrop undimmed — the app behind
        // (bottom nav, mini player) showed straight through below short
        // content. This pins both regardless of style resolution.
        sheetView?.background =
            androidx.appcompat.content.res.AppCompatResources.getDrawable(
                requireContext(),
                R.drawable.menu_sheet_bg,
            )
        d.window?.addFlags(android.view.WindowManager.LayoutParams.FLAG_DIM_BEHIND)
        d.window?.setDimAmount(0.5f)
    }

    private sealed interface Row {
        data class Header(val target: MenuTarget) : Row
        data class Action(val title: String, val icon: Int, val run: () -> Unit) : Row
        data class Section(val title: String) : Row
        data class Nav(
            var cover: String,
            val title: String,
            val sub: String,
            val kind: String,
            val id: String,
        ) : Row
    }

    private inner class Adapter(
        private val t: MenuTarget,
        private val close: () -> Unit,
    ) : RecyclerView.Adapter<RecyclerView.ViewHolder>() {
        // Action scope outlives the sheet: caller fragment's scope when the
        // sheet was shown from one (player-queue menus have no parent
        // fragment), else the activity scope. Either way a dismiss cannot
        // cancel the fetch behind Play/Download.
        private val scope: CoroutineScope =
            parentFragment?.lifecycleScope ?: requireActivity().lifecycleScope
        private val nav: (kind: String, id: String, title: String) -> Unit = { kind, id, title ->
            (activity as? MainActivity)?.openDetail(kind, id, title)
        }
        private val rows: List<Row> = buildList {
            add(Row.Header(t))
            fun act(title: String, icon: Int, fn: () -> Unit) {
                add(Row.Action(title, icon) { close(); fn() })
            }
            act("Play", R.drawable.ic_menu_play) { ContextMenuActions.play(scope, t) }
            act("Add to next", R.drawable.ic_menu_next) { ContextMenuActions.addNext(scope, t) }
            act("Add to queue", R.drawable.ic_menu_queue) { ContextMenuActions.enqueue(scope, t) }
            act("Save to playlist", R.drawable.ic_menu_playlist_add) { ContextMenuActions.saveToPlaylist() }
            act("Download", R.drawable.ic_menu_download) {
                val ctx = context ?: return@act
                ContextMenuActions.download(scope, ctx, t)
            }
            act("Save to library", R.drawable.ic_menu_library) { ContextMenuActions.saveToLibrary() }
            act("Like", R.drawable.ic_menu_like) { ContextMenuActions.like() }
            act("Share", R.drawable.ic_menu_share) {
                val ctx = context ?: return@act
                ContextMenuActions.share(ctx, t)
            }
            val artists = t.artistNav()
            if (artists.isNotEmpty()) {
                add(Row.Section(requireContext().getString(R.string.menu_artists)))
                artists.forEach { (name, id) ->
                    add(Row.Nav("", name, "Artist", "artist", id))
                }
            }
            t.albumNav()?.let { (name, id) ->
                add(Row.Section(requireContext().getString(R.string.menu_album)))
                add(Row.Nav(t.header().first, name, "Album", "album", id))
            }
        }

        init {
            // Artist rows arrive without covers (tracks only carry
            // artist names + IDs) — backfill from the cached artist page
            // and rebind just those rows. Dismissal cancels the scope, so
            // a closed sheet never touches a dead adapter.
            this@ContextMenuSheet.lifecycleScope.launch {
                rows.forEachIndexed { i, r ->
                    if (r is Row.Nav && r.kind == "artist" && r.cover.isEmpty()) {
                        val url = MusicRepository.artistPage(r.id).getOrNull()
                            ?.optJSONObject("artist")?.let { Models.artist(it).imageUrl }
                            .orEmpty()
                        if (url.isNotEmpty()) {
                            r.cover = url
                            notifyItemChanged(i)
                        }
                    }
                }
            }
        }

        fun isAction(pos: Int): Boolean = rows[pos] is Row.Action

        override fun getItemCount(): Int = rows.size

        override fun getItemViewType(pos: Int): Int = when (rows[pos]) {
            is Row.Header -> 0
            is Row.Action -> 1
            is Row.Section -> 2
            is Row.Nav -> 3
        }

        override fun onCreateViewHolder(parent: ViewGroup, viewType: Int): RecyclerView.ViewHolder {
            val inf = LayoutInflater.from(parent.context)
            return when (viewType) {
                0 -> HeaderHolder(inf.inflate(R.layout.item_context_header, parent, false))
                1 -> ActionHolder(inf.inflate(R.layout.item_context_action, parent, false))
                2 -> SectionHolder(inf.inflate(R.layout.item_context_section, parent, false))
                else -> NavHolder(inf.inflate(R.layout.item_title, parent, false))
            }
        }

        override fun onBindViewHolder(h: RecyclerView.ViewHolder, pos: Int) {
            when (val r = rows[pos]) {
                is Row.Header -> (h as HeaderHolder).bind(r.target)
                is Row.Action -> (h as ActionHolder).bind(r)
                is Row.Section -> (h as SectionHolder).bind(r.title)
                is Row.Nav -> (h as NavHolder).bind(r)
            }
        }

        private inner class HeaderHolder(private val v: View) : RecyclerView.ViewHolder(v) {
            private val thumb: ImageView = v.findViewById(R.id.menu_thumb)
            private val title: TextView = v.findViewById(R.id.menu_title)
            private val sub: TextView = v.findViewById(R.id.menu_sub)

            fun bind(t: MenuTarget) {
                val (cover, name, subText) = t.header()
                ArtworkLoader.load(thumb, cover, ArtworkLoader.Art.PLAYER)
                title.text = name.ifEmpty { "Unknown" }
                sub.text = subText
                sub.visibility = if (subText.isEmpty()) View.GONE else View.VISIBLE
                v.findViewById<ImageButton>(R.id.menu_close)?.setOnClickListener { close() }
            }
        }

        private inner class ActionHolder(private val v: View) : RecyclerView.ViewHolder(v) {
            fun bind(r: Row.Action) {
                v.findViewById<ImageView>(R.id.action_icon)?.setImageResource(r.icon)
                v.findViewById<TextView>(R.id.action_label)?.text = r.title
                v.setOnClickListener { r.run() }
            }
        }

        private inner class SectionHolder(private val v: View) : RecyclerView.ViewHolder(v) {
            fun bind(title: String) {
                (v as? TextView)?.text = title
            }
        }

        private inner class NavHolder(private val v: View) : RecyclerView.ViewHolder(v) {
            fun bind(r: Row.Nav) {
                // item_title's own dots would recurse into menus — hidden here.
                v.findViewById<View>(R.id.title_more)?.visibility = View.GONE
                val art: ImageView? = v.findViewById(R.id.title_art)
                if (art != null) {
                    ArtworkLoader.load(art, r.cover)
                    if (r.kind == "artist") Design.clipCircle(art)
                }
                v.findViewById<TextView>(R.id.title_main)?.text = r.title
                v.findViewById<TextView>(R.id.title_sub)?.text = r.sub
                v.setOnClickListener {
                    close()
                    nav(r.kind, r.id, r.title)
                }
            }
        }
    }
}
