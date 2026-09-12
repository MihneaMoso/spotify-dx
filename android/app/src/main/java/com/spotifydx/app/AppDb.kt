package com.spotifydx.app

import android.content.Context
import androidx.room.ColumnInfo
import androidx.room.Dao
import androidx.room.Database
import androidx.room.Entity
import androidx.room.Insert
import androidx.room.OnConflictStrategy
import androidx.room.PrimaryKey
import androidx.room.Query
import androidx.room.Room
import androidx.room.RoomDatabase
import androidx.room.Transaction

/**
 * Persisted app state (Room owns the SQL; DAOs are suspend — no hand-rolled
 * SQLite, no manual JSON files):
 *
 * - `search_history`: recent queries (LRU-capped) for instant repeat search.
 * - `queue_items`: ordered track snapshots — the queue survives restarts.
 * - `history_items`: played-track snapshots, oldest→newest — Echo-style past
 *   songs so users can jump back (capped, survives restarts).
 * - `cache_entries`: audio-cache index (bytes, completeness, play stats) —
 *   the files themselves live in filesDir/audiocache (AudioCache).
 * - `playback_state`: single row — last-played track + position + wall-clock
 *   timestamp, restored paused (never autoplay).
 */
@Entity(tableName = "search_history")
data class SearchEntry(
    @PrimaryKey val query: String,
    @ColumnInfo(name = "last_used_ms") val lastUsedMs: Long,
    @ColumnInfo(name = "use_count") val useCount: Int,
)

@Entity(tableName = "queue_items")
data class QueueItem(
    @PrimaryKey val pos: Int,
    @ColumnInfo(name = "track_id") val trackId: String,
    @ColumnInfo(name = "track_json") val trackJson: String,
)

@Entity(tableName = "history_items")
data class HistoryItem(
    @PrimaryKey val pos: Int,
    @ColumnInfo(name = "track_id") val trackId: String,
    @ColumnInfo(name = "track_json") val trackJson: String,
)

@Entity(tableName = "cache_entries")
data class CacheEntry(
    @PrimaryKey @ColumnInfo(name = "track_id") val trackId: String,
    /** Provider/format/quality tag — a quality change invalidates the file. */
    @ColumnInfo(name = "quality_key") val qualityKey: String = "",
    @ColumnInfo(name = "bytes") val bytes: Long = 0,
    /** Total length when known from upstream (-1 unknown). */
    @ColumnInfo(name = "total_bytes") val totalBytes: Long = -1,
    @ColumnInfo(name = "complete") val complete: Boolean = false,
    @ColumnInfo(name = "play_count") val playCount: Int = 0,
    @ColumnInfo(name = "last_played_ms") val lastPlayedMs: Long = 0,
)

@Entity(tableName = "playback_state")
data class PlaybackStateRow(
    @PrimaryKey val id: Int = 0,
    @ColumnInfo(name = "track_json") val trackJson: String?,
    @ColumnInfo(name = "position_ms") val positionMs: Long,
    @ColumnInfo(name = "updated_at_ms") val updatedAtMs: Long,
    /** Play origin label ("Liked Songs", playlist name, "Queue"…). v2+. */
    @ColumnInfo(name = "source", defaultValue = "") val source: String = "",
)

@Dao
interface SearchDao {
    @Query("SELECT * FROM search_history ORDER BY last_used_ms DESC LIMIT :limit")
    suspend fun recent(limit: Int): List<SearchEntry>

    @Query("SELECT use_count FROM search_history WHERE `query` = :q")
    suspend fun count(q: String): Int?

    @Insert(onConflict = OnConflictStrategy.REPLACE)
    suspend fun upsert(e: SearchEntry)

    @Query(
        "DELETE FROM search_history WHERE `query` NOT IN " +
            "(SELECT `query` FROM search_history ORDER BY last_used_ms DESC LIMIT :keep)",
    )
    suspend fun trim(keep: Int)

    @Query("DELETE FROM search_history")
    suspend fun clear()
}

@Dao
interface QueueDao {
    @Query("SELECT * FROM queue_items ORDER BY pos")
    suspend fun all(): List<QueueItem>

    @Insert(onConflict = OnConflictStrategy.REPLACE)
    suspend fun putAll(items: List<QueueItem>)

    @Query("DELETE FROM queue_items")
    suspend fun clear()

    @Transaction
    suspend fun replace(items: List<QueueItem>) {
        clear()
        putAll(items)
    }
}

@Dao
interface HistoryDao {
    @Query("SELECT * FROM history_items ORDER BY pos")
    suspend fun all(): List<HistoryItem>

    @Insert(onConflict = OnConflictStrategy.REPLACE)
    suspend fun putAll(items: List<HistoryItem>)

    @Query("DELETE FROM history_items")
    suspend fun clear()

    @Transaction
    suspend fun replace(items: List<HistoryItem>) {
        clear()
        putAll(items)
    }
}

@Dao
interface AudioCacheDao {
    @Query("SELECT * FROM cache_entries WHERE track_id = :key")
    suspend fun entry(key: String): CacheEntry?

    @Insert(onConflict = OnConflictStrategy.REPLACE)
    suspend fun upsert(e: CacheEntry)

    @Query("DELETE FROM cache_entries WHERE track_id = :key")
    suspend fun delete(key: String)

    @Query("SELECT * FROM cache_entries ORDER BY last_played_ms")
    suspend fun allOrdered(): List<CacheEntry>

    @Query("SELECT COALESCE(SUM(bytes), 0) FROM cache_entries")
    suspend fun totalBytes(): Long

    @Query("DELETE FROM cache_entries")
    suspend fun clear()
}

@Dao
interface PlaybackDao {
    @Query("SELECT * FROM playback_state WHERE id = 0")
    suspend fun get(): PlaybackStateRow?

    @Insert(onConflict = OnConflictStrategy.REPLACE)
    suspend fun save(r: PlaybackStateRow)

    @Query("DELETE FROM playback_state")
    suspend fun clear()
}

@Database(
    entities = [SearchEntry::class, QueueItem::class, HistoryItem::class, CacheEntry::class, PlaybackStateRow::class],
    version = 4,
    exportSchema = false,
)
abstract class AppDb : RoomDatabase() {
    abstract fun search(): SearchDao
    abstract fun queue(): QueueDao
    abstract fun history(): HistoryDao
    abstract fun audioCache(): AudioCacheDao
    abstract fun playback(): PlaybackDao

    companion object {
        const val HISTORY_KEEP = 20

        /** v1 → v2: play-origin label on the last-played row. */
        val MIGRATION_1_2 = object : androidx.room.migration.Migration(1, 2) {
            override fun migrate(db: androidx.sqlite.db.SupportSQLiteDatabase) {
                db.execSQL("ALTER TABLE playback_state ADD COLUMN source TEXT NOT NULL DEFAULT ''")
            }
        }

        /** v2 → v3: played-history table (Echo-style past songs). */
        val MIGRATION_2_3 = object : androidx.room.migration.Migration(2, 3) {
            override fun migrate(db: androidx.sqlite.db.SupportSQLiteDatabase) {
                db.execSQL(
                    "CREATE TABLE IF NOT EXISTS history_items (" +
                        "pos INTEGER NOT NULL PRIMARY KEY, " +
                        "track_id TEXT NOT NULL, " +
                        "track_json TEXT NOT NULL)",
                )
            }
        }

        /** v3 → v4: audio-cache index (files live in filesDir/audiocache). */
        val MIGRATION_3_4 = object : androidx.room.migration.Migration(3, 4) {
            override fun migrate(db: androidx.sqlite.db.SupportSQLiteDatabase) {
                db.execSQL(
                    "CREATE TABLE IF NOT EXISTS cache_entries (" +
                        "track_id TEXT NOT NULL PRIMARY KEY, " +
                        "quality_key TEXT NOT NULL DEFAULT '', " +
                        "bytes INTEGER NOT NULL DEFAULT 0, " +
                        "total_bytes INTEGER NOT NULL DEFAULT -1, " +
                        "complete INTEGER NOT NULL DEFAULT 0, " +
                        "play_count INTEGER NOT NULL DEFAULT 0, " +
                        "last_played_ms INTEGER NOT NULL DEFAULT 0)",
                )
            }
        }

        @Volatile
        private var inst: AppDb? = null

        fun get(ctx: Context): AppDb =
            inst ?: synchronized(this) {
                inst ?: Room.databaseBuilder(
                    ctx.applicationContext,
                    AppDb::class.java,
                    "spotifydx.db",
                ).addMigrations(MIGRATION_1_2, MIGRATION_2_3, MIGRATION_3_4).build().also { inst = it }
            }
    }
}
