package com.spotifydx.app

import androidx.lifecycle.ViewModel
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.async
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

/**
 * One ViewModel per screen (§7 migration). No business logic in fragments:
 * screens render state and forward intents; ViewModels own pagination,
 * retry, and refresh; the core owns data.
 *
 * NOTE: `viewModelScope`/`by viewModels()` live in `-ktx` artifacts outside
 * the offline cache, so ViewModels extend [ScopedViewModel] (same
 * viewModelScope semantics: SupervisorJob + Main.immediate, cancelled in
 * onCleared) and fragments use `ViewModelProvider` directly.
 */
open class ScopedViewModel : ViewModel() {
    protected val vmScope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    override fun onCleared() {
        vmScope.cancel()
        super.onCleared()
    }
}

// -- Gate -----------------------------------------------------------------------
class LoginViewModel : ScopedViewModel() {
    private val _starting = MutableStateFlow(false)
    val starting: StateFlow<Boolean> = _starting.asStateFlow()
    private val _loginUrl = MutableStateFlow<String?>(null)
    val loginUrl: StateFlow<String?> = _loginUrl.asStateFlow()

    /** Ask the core to begin login (§8.1): already-authenticated short-circuits. */
    fun begin() {
        if (_starting.value) return
        _starting.value = true
        vmScope.launch {
            val res = BridgeClient.beginLogin()
            val json = res.getOrNull()
            if (json != null && json.optBoolean("authenticated", false)) {
                // Live mirror (reopen with surviving process, or a logout
                // that raced a still-valid token): never stick on
                // "Opening login…" — drop the spinner and re-emit the
                // session snapshot so the gate→shell switch fires.
                _starting.value = false
                SessionRepository.refresh()
            } else if (json != null) {
                val url = if (json.isNull("login_url")) null else json.getString("login_url")
                if (url == null) {
                    // Not authenticated and no page to show: surface retry,
                    // never a stuck spinner.
                    _starting.value = false
                    ToastBus.error("Sign-in unavailable — please retry.")
                } else {
                    _loginUrl.value = url
                }
            } else {
                _starting.value = false
                ToastBus.fromBridge(res.exceptionOrNull() ?: return@launch)
            }
        }
    }

    fun consumedUrl() {
        _loginUrl.value = null
    }

    fun reset() {
        _starting.value = false
        _loginUrl.value = null
    }
}

// -- Home ------------------------------------------------------------------------
class HomeViewModel : ScopedViewModel() {
    private val _state = MutableStateFlow<ScreenState>(ScreenState.Loading)
    val state: StateFlow<ScreenState> = _state.asStateFlow()
    private val _banner = MutableStateFlow<String?>(null)
    val banner: StateFlow<String?> = _banner.asStateFlow()
    private val _playlists = MutableStateFlow<List<Playlist>>(emptyList())
    val playlists: StateFlow<List<Playlist>> = _playlists.asStateFlow()
    private val _liked = MutableStateFlow<List<Track>>(emptyList())
    val liked: StateFlow<List<Track>> = _liked.asStateFlow()

    private var retryJob: Job? = null

    val greeting: String
        get() = when (java.util.Calendar.getInstance().get(java.util.Calendar.HOUR_OF_DAY)) {
            in 5..11 -> "Good morning"
            in 12..17 -> "Good afternoon"
            in 18..22 -> "Good evening"
            else -> "Good night"
        }

    fun load() {
        retryJob?.cancel()
        _state.value = ScreenState.Loading
        vmScope.launch {
            val res = MusicRepository.withSessionCheck { MusicRepository.home() }
            val err = res.exceptionOrNull()
            val json = res.getOrNull()
            if (json != null) {
                _banner.value = null
                _playlists.value = run {
                    val arr = json.optJSONArray("playlists") ?: org.json.JSONArray()
                    List(arr.length()) { i -> arr.optJSONObject(i) }
                        .filterNotNull().map(Models::playlist)
                }
                _liked.value = Models.tracks(json.optJSONArray("liked_tracks"))
                _state.value = ScreenState.Content(
                    empty = _playlists.value.isEmpty() && _liked.value.isEmpty(),
                )
            } else if (isRateLimited(err)) {
                // Banner + 60s timed auto-retry until quota clears (§7.5).
                _banner.value = "Spotify's API is temporarily limiting requests — retrying automatically."
                _state.value = ScreenState.Content(empty = true)
                retryJob = launch {
                    delay(60_000)
                    load()
                }
            } else {
                _state.value = ScreenState.Error(UiStates.errorCopy(err)) { load() }
            }
        }
    }

    private fun isRateLimited(e: Throwable?): Boolean {
        val err = (e as? BridgeException)?.error
        return err is BridgeError.RateLimited ||
            (err is BridgeError.Core && err.message.contains("rate", ignoreCase = true))
    }
}

// -- Search -----------------------------------------------------------------------
class SearchViewModel : ScopedViewModel() {
    private val _state = MutableStateFlow<ScreenState>(ScreenState.Content(empty = true))
    val state: StateFlow<ScreenState> = _state.asStateFlow()
    private val _tracks = MutableStateFlow<List<Track>>(emptyList())
    val tracks: StateFlow<List<Track>> = _tracks.asStateFlow()
    private val _albums = MutableStateFlow<List<Album>>(emptyList())
    val albums: StateFlow<List<Album>> = _albums.asStateFlow()
    private val _artists = MutableStateFlow<List<Artist>>(emptyList())
    val artists: StateFlow<List<Artist>> = _artists.asStateFlow()

    private var generation = 0
    private var debounce: Job? = null
    private var handoff: String? = null

    private val _recent = MutableStateFlow<List<String>>(emptyList())
    val recent: StateFlow<List<String>> = _recent.asStateFlow()

    init {
        vmScope.launch { _recent.value = SearchHistory.recent() }
    }

    fun clearHistory() {
        vmScope.launch {
            SearchHistory.clear()
            _recent.value = emptyList()
        }
    }

    /** One-shot top-bar handoff: consumed on arrival, never re-seeded. */
    fun consumeHandoff(query: String?) {
        handoff = query
        query?.takeIf { it.isNotBlank() }?.let { submit(it) }
        handoff = null
    }

    /** 250ms debounce + staleness guard against out-of-order responses. */
    fun submit(query: String) {
        debounce?.cancel()
        if (query.isBlank()) {
            _tracks.value = emptyList()
            _albums.value = emptyList()
            _artists.value = emptyList()
            _state.value = ScreenState.Content(empty = true)
            return
        }
        debounce = vmScope.launch {
            delay(250)
            val g = ++generation
            _state.value = ScreenState.Loading
            // Persist first (fire-and-forget): history survives even a failed fetch.
            SearchHistory.record(query)
            _recent.value = SearchHistory.recent()
            val res = MusicRepository.withSessionCheck { MusicRepository.search(query) }
            if (g != generation) return@launch
            val json = res.getOrNull()
            if (json != null) {
                _tracks.value = Models.tracks(json.optJSONObject("tracks")?.optJSONArray("items"))
                _albums.value = run {
                    val arr = json.optJSONObject("albums")?.optJSONArray("items")
                        ?: org.json.JSONArray()
                    List(arr.length()) { i -> arr.optJSONObject(i) }
                        .filterNotNull().map(Models::album)
                }
                _artists.value = run {
                    val arr = json.optJSONObject("artists")?.optJSONArray("items")
                        ?: org.json.JSONArray()
                    List(arr.length()) { i -> arr.optJSONObject(i) }
                        .filterNotNull().map(Models::artist)
                }
                val empty = _tracks.value.isEmpty() && _albums.value.isEmpty() &&
                    _artists.value.isEmpty()
                _state.value = ScreenState.Content(empty = empty)
            } else {
                _state.value = ScreenState.Error(UiStates.errorCopy(res.exceptionOrNull())) {
                    submit(query)
                }
            }
        }
    }
}

// -- Library -----------------------------------------------------------------------
class LibraryViewModel : ScopedViewModel() {
    enum class Tab { PLAYLISTS, ALBUMS, LIKED }

    private val _state = MutableStateFlow<ScreenState>(ScreenState.Loading)
    val state: StateFlow<ScreenState> = _state.asStateFlow()
    private val _tab = MutableStateFlow(Tab.PLAYLISTS)
    val tab: StateFlow<Tab> = _tab.asStateFlow()
    private val _filter = MutableStateFlow("")
    val filter: StateFlow<String> = _filter.asStateFlow()
    private val _rows = MutableStateFlow<List<LibraryRow>>(emptyList())
    val rows: StateFlow<List<LibraryRow>> = _rows.asStateFlow()

    private var playlists: List<Playlist> = emptyList()
    private var albums: List<Album> = emptyList()
    private var liked: List<Track> = emptyList()

    sealed interface LibraryRow {
        data class P(val p: Playlist) : LibraryRow
        data class A(val a: Album) : LibraryRow
        data class T(val t: Track) : LibraryRow
        fun name(): String = when (this) {
            is P -> p.name
            is A -> a.name
            is T -> t.name
        }
    }

    fun selectTab(t: Tab) {
        _tab.value = t
        apply()
    }

    fun setFilter(q: String) {
        _filter.value = q
        apply()
    }

    fun load() {
        vmScope.launch {
            _state.value = ScreenState.Loading
            // Concurrent trio; ANY leg failing surfaces instead of half UI.
            // Session failures flip the gate via withSessionCheck.
            val playlistsRes = async {
                MusicRepository.withSessionCheck { MusicRepository.library("playlists", 50, 0) }
            }
            val albumsRes = async {
                MusicRepository.withSessionCheck { MusicRepository.library("albums", 50, 0) }
            }
            val likedRes = async {
                MusicRepository.withSessionCheck { MusicRepository.likedTracks(50, 0) }
            }
            val p = playlistsRes.await()
            val a = albumsRes.await()
            val l = likedRes.await()
            val firstErr = listOf(p, a, l).firstOrNull { it.isFailure }?.exceptionOrNull()
            if (firstErr != null) {
                _state.value = ScreenState.Error(UiStates.errorCopy(firstErr)) { load() }
                return@launch
            }
            playlists = run {
                val arr = p.getOrNull()?.optJSONArray("items") ?: org.json.JSONArray()
                List(arr.length()) { i -> arr.optJSONObject(i) }
                    .filterNotNull().map(Models::playlist)
            }
            albums = run {
                val got = a.getOrNull()
                val arr = got?.optJSONArray("items") ?: org.json.JSONArray()
                List(arr.length()) { i -> arr.optJSONObject(i) }
                    .filterNotNull().map(Models::album)
            }
            liked = Models.savedTracks(l.getOrNull() ?: org.json.JSONObject())
            apply()
        }
    }

    private fun apply() {
        // Client-side text filter + A–Z sort over the loaded snapshot.
        val q = _filter.value.trim().lowercase()
        fun matches(name: String) = q.isEmpty() || name.lowercase().contains(q)
        val rows: List<LibraryRow> = when (_tab.value) {
            Tab.PLAYLISTS -> playlists.filter { matches(it.name) }
                .sortedBy { it.name.lowercase() }.map(LibraryRow::P)
            Tab.ALBUMS -> albums.filter { matches(it.name) }
                .sortedBy { it.name.lowercase() }.map(LibraryRow::A)
            Tab.LIKED -> liked.filter { matches(it.name) }
                .sortedBy { it.name.lowercase() }.map(LibraryRow::T)
        }
        _rows.value = rows
        _state.value = ScreenState.Content(empty = rows.isEmpty())
    }
}

// -- Liked (paged, append-only) ------------------------------------------------------
class LikedViewModel : ScopedViewModel() {
    private val _state = MutableStateFlow<ScreenState>(ScreenState.Loading)
    val state: StateFlow<ScreenState> = _state.asStateFlow()
    private val _tracks = MutableStateFlow<List<Track>>(emptyList())
    val tracks: StateFlow<List<Track>> = _tracks.asStateFlow()

    private var offset = 0
    private var loadingMore = false
    var total = Int.MAX_VALUE
        private set

    fun load() {
        offset = 0
        total = Int.MAX_VALUE
        _tracks.value = emptyList()
        loadPage()
    }

    fun loadMore() {
        if (loadingMore || _tracks.value.size >= total) return
        loadPage()
    }

    private fun loadPage() {
        loadingMore = true
        if (offset == 0) _state.value = ScreenState.Loading
        vmScope.launch {
            val res = MusicRepository.withSessionCheck { MusicRepository.likedTracks(50, offset) }
            loadingMore = false
            val json = res.getOrNull()
            if (json != null) {
                val page = Models.savedTracks(json)
                total = Models.pageTotal(json, _tracks.value.size + page.size)
                offset += page.size
                // Append-only accumulation owned by the ViewModel (§6.2).
                _tracks.value = _tracks.value + page
                _state.value = ScreenState.Content(empty = _tracks.value.isEmpty())
            } else if (offset == 0) {
                _state.value = ScreenState.Error(UiStates.errorCopy(res.exceptionOrNull())) { load() }
            } else {
                ToastBus.fromBridge(res.exceptionOrNull() ?: return@launch)
            }
        }
    }
}

// -- Detail (album / artist / playlist) ------------------------------------------------
class DetailViewModel : ScopedViewModel() {
    private val _state = MutableStateFlow<ScreenState>(ScreenState.Loading)
    val state: StateFlow<ScreenState> = _state.asStateFlow()
    private val _title = MutableStateFlow("")
    val title: StateFlow<String> = _title.asStateFlow()
    private val _subtitle = MutableStateFlow("")
    val subtitle: StateFlow<String> = _subtitle.asStateFlow()
    private val _tracks = MutableStateFlow<List<Track>>(emptyList())
    val tracks: StateFlow<List<Track>> = _tracks.asStateFlow()

    fun load(kind: String, id: String) {
        _state.value = ScreenState.Loading
        vmScope.launch {
            val res = MusicRepository.withSessionCheck {
                when (kind) {
                    "album" -> MusicRepository.album(id)
                    "artist" -> MusicRepository.artistPage(id)
                    else -> MusicRepository.playlist(id)
                }
            }
            val json = res.getOrNull()
            if (json != null) {
                when (kind) {
                    "album" -> {
                        val album = json.optJSONObject("album") ?: org.json.JSONObject()
                        _title.value = album.optString("name", "")
                        _subtitle.value = album.optString("release_date", "")
                        _tracks.value = Models.tracks(json.optJSONArray("tracks"))
                    }
                    "artist" -> {
                        val artist = json.optJSONObject("artist") ?: org.json.JSONObject()
                        _title.value = artist.optString("name", "")
                        _subtitle.value = "Artist"
                        _tracks.value = Models.tracks(json.optJSONArray("top_tracks"))
                    }
                    else -> {
                        _title.value = json.optString("name", "")
                        _subtitle.value = json.optString("description", "")
                        _tracks.value = Models.playlistTracks(json)
                    }
                }
                _state.value = ScreenState.Content(empty = _tracks.value.isEmpty())
            } else {
                _state.value = ScreenState.Error(UiStates.errorCopy(res.exceptionOrNull())) {
                    load(kind, id)
                }
            }
        }
    }
}
