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
    }
}
