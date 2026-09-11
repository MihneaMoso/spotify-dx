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
interface PlaybackDao {
    @Query("SELECT * FROM playback_state WHERE id = 0")
    suspend fun get(): PlaybackStateRow?

    @Insert(onConflict = OnConflictStrategy.REPLACE)
    suspend fun save(r: PlaybackStateRow)

    @Query("DELETE FROM playback_state")
    suspend fun clear()
}

@Database(
    entities = [SearchEntry::class, QueueItem::class, PlaybackStateRow::class],
    version = 2,
    exportSchema = false,
)
abstract class AppDb : RoomDatabase() {
    abstract fun search(): SearchDao
    abstract fun queue(): QueueDao
    abstract fun playback(): PlaybackDao

    companion object {
        const val HISTORY_KEEP = 20

        /** v1 → v2: play-origin label on the last-played row. */
        val MIGRATION_1_2 = object : androidx.room.migration.Migration(1, 2) {
            override fun migrate(db: androidx.sqlite.db.SupportSQLiteDatabase) {
                db.execSQL("ALTER TABLE playback_state ADD COLUMN source TEXT NOT NULL DEFAULT ''")
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
                ).addMigrations(MIGRATION_1_2).build().also { inst = it }
            }
    }
}
