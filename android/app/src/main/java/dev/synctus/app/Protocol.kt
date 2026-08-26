package dev.synctus.app

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json

/**
 * Kotlin mirrors of the JSON shapes the Rust bridge speaks.
 *
 * Field names match the Rust `serde` names exactly; anything that drifts shows up
 * as a decode failure rather than a silently missing value, because
 * [SynctusJson] is configured to ignore unknown keys but not to invent missing
 * ones.
 */
val SynctusJson = Json {
    ignoreUnknownKeys = true
    encodeDefaults = true
    explicitNulls = false
}

// --- outgoing: commands -----------------------------------------------------

@Serializable
sealed interface BridgeCommand {

    @Serializable
    @SerialName("publish")
    data class Publish(
        val presence: String? = null,
        val foreground: ForegroundApp? = null,
        val battery: Battery? = null,
        val music: NowPlaying? = null,
    ) : BridgeCommand

    @Serializable
    @SerialName("nudge")
    data class Nudge(val kind: String) : BridgeCommand

    @Serializable
    @SerialName("set_presence")
    data class SetPresence(val presence: String) : BridgeCommand

    @Serializable
    @SerialName("toggle_pomodoro")
    data object TogglePomodoro : BridgeCommand

    @Serializable
    @SerialName("start_focus")
    data object StartFocus : BridgeCommand

    @Serializable
    @SerialName("stop_pomodoro")
    data object StopPomodoro : BridgeCommand

    @Serializable
    @SerialName("skip_phase")
    data object SkipPhase : BridgeCommand

    @Serializable
    @SerialName("set_todos")
    data class SetTodos(val items: List<Todo>) : BridgeCommand

    @Serializable
    @SerialName("reconfigure")
    data class Reconfigure(val config: ClientConfig) : BridgeCommand
}

// --- incoming: events -------------------------------------------------------

@Serializable
sealed interface BridgeEvent {

    @Serializable
    @SerialName("connection")
    data class Connection(val state: String, val detail: String = "") : BridgeEvent

    @Serializable
    @SerialName("peer")
    data class Peer(
        val name: String,
        val platform: String,
        val presence: String,
        @SerialName("presence_color") val presenceColor: Long,
        val detail: String,
        val meta: String,
        val stale: Boolean,
    ) : BridgeEvent

    @Serializable
    @SerialName("nudge")
    data class Nudge(val title: String, val body: String, val kind: String) : BridgeEvent

    @Serializable
    @SerialName("pomodoro")
    data class Pomodoro(
        val phase: String,
        val remaining: String,
        val finished: Boolean,
        val message: String,
    ) : BridgeEvent

    @Serializable
    @SerialName("peer_todos")
    data class PeerTodos(val items: List<Todo>) : BridgeEvent

    @Serializable
    @SerialName("warning")
    data class Warning(val message: String) : BridgeEvent
}

// --- shared models ----------------------------------------------------------

@Serializable
data class ForegroundApp(
    val app: String,
    val name: String? = null,
    val title: String? = null,
)

@Serializable
data class Battery(
    val percent: Int,
    val charging: Boolean,
    @SerialName("minutes_left") val minutesLeft: Int? = null,
)

@Serializable
data class NowPlaying(
    val title: String,
    val artist: String? = null,
    val album: String? = null,
    val player: String? = null,
    val playing: Boolean,
)

@Serializable
data class Todo(
    val id: String,
    val title: String,
    val done: Boolean = false,
    @SerialName("created_at") val createdAt: Long,
    @SerialName("done_at") val doneAt: Long? = null,
    val pomodoros: Int = 0,
)

/** Local status, used to render the foreground notification. */
@Serializable
data class LocalStatus(
    val presence: String = "在忙",
    @SerialName("pomodoro_phase") val pomodoroPhase: String = "未开始",
    @SerialName("pomodoro_remaining") val pomodoroRemaining: String = "00:00",
    @SerialName("pomodoro_active") val pomodoroActive: Boolean = false,
    @SerialName("pomodoro_paused") val pomodoroPaused: Boolean = false,
    @SerialName("completed_today") val completedToday: Int = 0,
    val connected: Boolean = false,
)

// --- configuration ----------------------------------------------------------

@Serializable
data class Privacy(
    @SerialName("share_foreground_app") val shareForegroundApp: Boolean = true,
    @SerialName("share_window_title") val shareWindowTitle: Boolean = false,
    @SerialName("share_battery") val shareBattery: Boolean = true,
    @SerialName("share_music") val shareMusic: Boolean = true,
    @SerialName("share_pomodoro") val sharePomodoro: Boolean = true,
    @SerialName("share_todos") val shareTodos: Boolean = true,
    @SerialName("share_idle") val shareIdle: Boolean = true,
    @SerialName("blocked_apps") val blockedApps: List<String> = emptyList(),
)

@Serializable
data class PomodoroConfig(
    @SerialName("focus_min") val focusMin: Int = 25,
    @SerialName("short_break_min") val shortBreakMin: Int = 5,
    @SerialName("long_break_min") val longBreakMin: Int = 15,
    @SerialName("rounds_per_set") val roundsPerSet: Int = 4,
    @SerialName("auto_continue") val autoContinue: Boolean = false,
    @SerialName("presence_follows_phase") val presenceFollowsPhase: Boolean = true,
)

/**
 * Mirrors the Rust `ClientConfig`. Defaults are duplicated here so the settings
 * screen can render before the native library has been asked for anything.
 */
@Serializable
data class ClientConfig(
    val server: String = "127.0.0.1:8787",
    val tls: Boolean = true,
    @SerialName("tls_server_name") val tlsServerName: String? = null,
    @SerialName("invite_code") val inviteCode: String = "",
    @SerialName("device_id") val deviceId: String = "",
    @SerialName("device_name") val deviceName: String = "Android",
    val privacy: Privacy = Privacy(),
    val pomodoro: PomodoroConfig = PomodoroConfig(),
    @SerialName("poll_secs") val pollSecs: Long = 15,
    @SerialName("away_after_secs") val awayAfterSecs: Int = 0,
    @SerialName("peer_stale_secs") val peerStaleSecs: Long = 90,
    @SerialName("start_minimised") val startMinimised: Boolean = false,
    val autostart: Boolean = true,
    @SerialName("show_overlay") val showOverlay: Boolean = false,
    @SerialName("overlay_x") val overlayX: Int? = null,
    @SerialName("overlay_y") val overlayY: Int? = null,
    @SerialName("overlay_always_on_top") val overlayAlwaysOnTop: Boolean = false,
    @SerialName("check_updates") val checkUpdates: Boolean = true,
    @SerialName("update_repo") val updateRepo: String = "LANqed/Synctus",
    @SerialName("mute_nudges") val muteNudges: Boolean = false,
) {
    /** Mirrors the Rust rule: at least 8 alphanumeric characters. */
    fun isPaired(): Boolean = inviteCode.count { it.isLetterOrDigit() } >= 8
}

/** Presence values, matching the Rust `snake_case` serialisation. */
object PresenceKey {
    const val ACTIVE = "active"
    const val RESTING = "resting"
    const val AWAY = "away"
    const val BUSY = "busy"

    /** Ordered for the settings and notification pickers. */
    val selectable = listOf(
        ACTIVE to "在忙",
        RESTING to "休息中",
        BUSY to "免打扰",
        AWAY to "离开",
    )
}

/** Interaction kinds, matching the Rust `snake_case` serialisation. */
object NudgeKey {
    const val KNOCK = "knock"
    const val HUG = "hug"
    const val COFFEE = "coffee"
    const val REST = "rest"
    const val FOCUS_TOGETHER = "focus_together"

    val all = listOf(
        Triple(KNOCK, "👋", "敲一敲"),
        Triple(HUG, "🤗", "抱抱"),
        Triple(COFFEE, "☕", "请喝咖啡"),
        Triple(REST, "🛋", "去休息"),
        Triple(FOCUS_TOGETHER, "🍅", "一起专注"),
    )
}
