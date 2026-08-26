package dev.synctus.app

/**
 * The JNI surface of the Rust engine.
 *
 * Everything crosses the boundary as JSON strings: commands in, events out. That
 * keeps this file to a list of declarations and puts all the logic in Rust, where
 * it is shared with the desktop client.
 *
 * Native functions that can fail return an empty string on success and a
 * human-readable message otherwise, which [check] converts into an exception at
 * the call site that cares.
 */
object NativeBridge {

    /** Set when [load] succeeded, so callers can degrade instead of crashing. */
    @Volatile
    var available: Boolean = false
        private set

    /** Reason [load] failed, for display in the UI. */
    @Volatile
    var loadError: String? = null
        private set

    /**
     * Load `libsynctus.so`.
     *
     * Called from [SynctusApp.onCreate]. A failure here means the APK was built
     * without the native library for this ABI; the UI shows [loadError] instead of
     * crashing on first use.
     */
    fun load() {
        if (available) return
        try {
            System.loadLibrary("synctus")
            nativeInit()
            available = true
            loadError = null
        } catch (e: Throwable) {
            available = false
            loadError = e.message ?: e.toString()
        }
    }

    /** Start the engine with a JSON [ClientConfig]. */
    fun start(configJson: String): Result<Unit> = call { nativeStart(configJson) }

    /** Send a JSON command. */
    fun command(commandJson: String): Result<Unit> = call { nativeCommand(commandJson) }

    /** Pending events as a JSON array. Never throws. */
    fun poll(): String = if (available) nativePoll() ?: "[]" else "[]"

    /** Local status as a JSON object. Never throws. */
    fun localStatus(): String = if (available) nativeLocalStatus() ?: "{}" else "{}"

    fun stop() {
        if (available) nativeStop()
    }

    fun isRunning(): Boolean = available && nativeRunning()

    fun newInviteCode(): String =
        if (available) nativeNewInviteCode() ?: "" else ""

    fun defaultConfigJson(): String =
        if (available) nativeDefaultConfig() ?: "{}" else "{}"

    fun version(): String = if (available) nativeVersion() ?: "?" else "?"

    /** Turn the "empty string means success" convention into a [Result]. */
    private inline fun call(block: () -> String?): Result<Unit> {
        if (!available) {
            return Result.failure(IllegalStateException(loadError ?: "原生库未加载"))
        }
        val message = block()
        return if (message.isNullOrEmpty()) {
            Result.success(Unit)
        } else {
            Result.failure(RuntimeException(message))
        }
    }

    // --- native declarations ------------------------------------------------

    private external fun nativeInit()

    private external fun nativeStart(configJson: String): String?

    private external fun nativeCommand(commandJson: String): String?

    private external fun nativePoll(): String?

    private external fun nativeLocalStatus(): String?

    private external fun nativeStop()

    private external fun nativeRunning(): Boolean

    private external fun nativeNewInviteCode(): String?

    private external fun nativeDefaultConfig(): String?

    private external fun nativeVersion(): String?
}
