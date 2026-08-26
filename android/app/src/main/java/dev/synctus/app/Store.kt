package dev.synctus.app

import android.content.Context
import android.content.SharedPreferences
import kotlinx.serialization.encodeToString

/**
 * Persisted settings and to-dos.
 *
 * `SharedPreferences` rather than a database: this is one config object and a
 * short list, and the synchronous read on startup is what lets [SyncService] start
 * the engine in `onCreate` without a coroutine.
 */
class Store(context: Context) {

    private val prefs: SharedPreferences =
        context.getSharedPreferences(NAME, Context.MODE_PRIVATE)

    /**
     * Load the config, falling back to defaults when absent or unreadable.
     *
     * A corrupt blob is replaced rather than propagated: losing settings is
     * recoverable, refusing to start is not.
     */
    fun loadConfig(): ClientConfig {
        val raw = prefs.getString(KEY_CONFIG, null) ?: return freshConfig()
        return try {
            SynctusJson.decodeFromString<ClientConfig>(raw)
        } catch (e: Exception) {
            freshConfig()
        }
    }

    fun saveConfig(config: ClientConfig) {
        prefs.edit()
            .putString(KEY_CONFIG, SynctusJson.encodeToString(config))
            .apply()
    }

    fun loadTodos(): List<Todo> {
        val raw = prefs.getString(KEY_TODOS, null) ?: return emptyList()
        return try {
            SynctusJson.decodeFromString<List<Todo>>(raw)
        } catch (e: Exception) {
            emptyList()
        }
    }

    fun saveTodos(todos: List<Todo>) {
        prefs.edit()
            .putString(KEY_TODOS, SynctusJson.encodeToString(todos))
            .apply()
    }

    /**
     * Whether the service should come back after a reboot. Stored separately from
     * the config so [BootReceiver] can read one boolean without decoding JSON.
     */
    var autostart: Boolean
        get() = prefs.getBoolean(KEY_AUTOSTART, true)
        set(value) = prefs.edit().putBoolean(KEY_AUTOSTART, value).apply()

    // --- daily focus accounting -------------------------------------------

    /**
     * Today's focus progress, rolled over at midnight.
     *
     * The engine holds these in memory only; Android owns the persistence because
     * its process can be killed and restarted at any time. Reading rolls the day
     * first, so a session that spans midnight starts fresh.
     */
    fun loadProgress(): Progress {
        val today = todayKey()
        val storedDate = prefs.getString(KEY_PROGRESS_DATE, null)

        if (storedDate != today) {
            // New day: minutes reset. The streak is kept — whether it survived
            // depends on when the goal was last met, which [registerGoalMet]
            // decides.
            return Progress(
                focusTodayMin = 0,
                streakDays = effectiveStreak(),
            )
        }

        return Progress(
            focusTodayMin = prefs.getInt(KEY_FOCUS_MIN, 0),
            streakDays = effectiveStreak(),
        )
    }

    /** Persist the engine's current totals. */
    fun saveProgress(focusTodayMin: Int, streakDays: Int) {
        prefs.edit()
            .putString(KEY_PROGRESS_DATE, todayKey())
            .putInt(KEY_FOCUS_MIN, focusTodayMin)
            .putInt(KEY_STREAK, streakDays)
            .apply()
    }

    /**
     * Record that today's goal was met, returning the new streak.
     *
     * Idempotent within a day: calling it twice does not inflate the streak, which
     * matters because the engine reports the crossing and the service may replay
     * events after a restart.
     */
    fun registerGoalMet(): Int {
        val today = todayKey()
        if (prefs.getString(KEY_GOAL_DATE, null) == today) {
            return prefs.getInt(KEY_STREAK, 0)
        }

        // A streak continues only if the previous success was yesterday.
        val previous = prefs.getString(KEY_GOAL_DATE, null)
        val streak = if (previous == yesterdayKey()) {
            prefs.getInt(KEY_STREAK, 0) + 1
        } else {
            1
        }

        prefs.edit()
            .putString(KEY_GOAL_DATE, today)
            .putInt(KEY_STREAK, streak)
            .apply()
        return streak
    }

    /**
     * The streak as it stands now.
     *
     * A stored streak goes stale: if the last success was three days ago the streak
     * is over, even though the number is still in the file. Today counts as intact
     * so an unfinished day does not read as a break.
     */
    private fun effectiveStreak(): Int {
        val last = prefs.getString(KEY_GOAL_DATE, null) ?: return 0
        return if (last == todayKey() || last == yesterdayKey()) {
            prefs.getInt(KEY_STREAK, 0)
        } else {
            0
        }
    }

    /** Today's focus totals. */
    data class Progress(val focusTodayMin: Int, val streakDays: Int)

    /**
     * Day key as days-since-epoch in UTC.
     *
     * Matches the Rust side's boundary exactly; using the device's local calendar
     * here would make the two disagree about when "today" ends.
     */
    private fun todayKey(): String =
        (System.currentTimeMillis() / 86_400_000L).toString()

    private fun yesterdayKey(): String =
        ((System.currentTimeMillis() - 86_400_000L) / 86_400_000L).toString()

    /**
     * A fresh config with a stable device id.
     *
     * The id is generated once and kept, because the peer uses it to tell one of
     * your devices from another; regenerating it would look like a new device on
     * every launch.
     */
    private fun freshConfig(): ClientConfig {
        val existing = prefs.getString(KEY_DEVICE_ID, null)
        val deviceId = existing ?: randomDeviceId().also {
            prefs.edit().putString(KEY_DEVICE_ID, it).apply()
        }
        return ClientConfig(
            deviceId = deviceId,
            deviceName = android.os.Build.MODEL ?: "Android",
        )
    }

    private fun randomDeviceId(): String {
        val bytes = ByteArray(8)
        java.security.SecureRandom().nextBytes(bytes)
        return bytes.joinToString("") { "%02x".format(it) }
    }

    private companion object {
        const val NAME = "synctus"
        const val KEY_CONFIG = "config"
        const val KEY_TODOS = "todos"
        const val KEY_AUTOSTART = "autostart"
        const val KEY_DEVICE_ID = "device_id"
        const val KEY_PROGRESS_DATE = "progress_date"
        const val KEY_FOCUS_MIN = "focus_today_min"
        const val KEY_STREAK = "streak_days"
        const val KEY_GOAL_DATE = "last_goal_date"
    }
}
