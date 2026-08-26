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
    data class Nudge(val kind: String, val text: String? = null) : BridgeCommand

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

    /**
     * Tell the engine where today's focus accounting left off.
     *
     * Android owns the persistence, so after a service restart the engine has to
     * be told rather than reading a file itself.
     */
    @Serializable
    @SerialName("restore_progress")
    data class RestoreProgress(
        @SerialName("focus_today_min") val focusTodayMin: Int,
        @SerialName("streak_days") val streakDays: Int,
    ) : BridgeCommand

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
        @SerialName("focus_today_min") val focusTodayMin: Int = 0,
        @SerialName("goal_min") val goalMin: Int = 0,
        @SerialName("streak_days") val streakDays: Int = 0,
        /** In a focus round right now. Gates the nag button. */
        val focusing: Boolean = false,
        /** Focusing on paper, but with a distracting app open. */
        val slacking: Boolean = false,
    ) : BridgeEvent {
        /** Progress towards their own goal, 0.0 to 1.0. */
        fun goalProgress(): Float =
            if (goalMin <= 0) 0f else (focusTodayMin.toFloat() / goalMin).coerceIn(0f, 1f)

        fun goalMet(): Boolean = goalMin > 0 && focusTodayMin >= goalMin
    }

    @Serializable
    @SerialName("nudge")
    data class Nudge(
        val title: String,
        val body: String,
        val kind: String,
        /** Whether it should break through do-not-disturb. */
        val urgent: Boolean = false,
    ) : BridgeEvent

    @Serializable
    @SerialName("pomodoro")
    data class Pomodoro(
        val phase: String,
        val remaining: String,
        val finished: Boolean,
        val message: String,
    ) : BridgeEvent

    /** The daily goal was reached; worth its own celebration. */
    @Serializable
    @SerialName("goal_reached")
    data class GoalReached(
        @SerialName("goal_min") val goalMin: Int,
        @SerialName("streak_days") val streakDays: Int,
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
    // --- accountability ---
    @SerialName("focus_today_min") val focusTodayMin: Int = 0,
    @SerialName("goal_min") val goalMin: Int = 0,
    @SerialName("streak_days") val streakDays: Int = 0,
    @SerialName("goal_met") val goalMet: Boolean = false,
    @SerialName("peer_focus_today_min") val peerFocusTodayMin: Int = 0,
    @SerialName("peer_focusing") val peerFocusing: Boolean = false,
    /** A distracting app is open during my own focus round. */
    val distracted: Boolean = false,
    @SerialName("distracted_by") val distractedBy: String? = null,
) {
    /** Progress towards my goal, 0.0 to 1.0. */
    fun goalProgress(): Float =
        if (goalMin <= 0) 0f else (focusTodayMin.toFloat() / goalMin).coerceIn(0f, 1f)

    /** Minutes still needed today. */
    fun remainingMin(): Int = (goalMin - focusTodayMin).coerceAtLeast(0)
}

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
 * The accountability settings — what turns a status widget into something that
 * actually keeps two people working.
 *
 * Defaults mirror the Rust side; a mismatch would silently change behaviour
 * between the settings screen and the engine.
 */
@Serializable
data class Accountability(
    /** Daily focus target in minutes. 0 disables goals and streaks. */
    @SerialName("daily_goal_min") val dailyGoalMin: Int = 100,
    /** Warn me when I open a distracting app during a focus round. */
    @SerialName("warn_on_distraction") val warnOnDistraction: Boolean = true,
    @SerialName("distracting_apps") val distractingApps: List<String> = defaultDistractingApps,
    @SerialName("distraction_grace_secs") val distractionGraceSecs: Int = 30,
    /** Off by default: being watched is something to opt into. */
    @SerialName("report_distraction_to_peer") val reportDistractionToPeer: Boolean = false,
    @SerialName("allow_urgent_nudges") val allowUrgentNudges: Boolean = true,
    @SerialName("auto_cheer") val autoCheer: Boolean = true,
) {
    companion object {
        /**
         * A starting list the user is expected to edit.
         *
         * Android package names rather than executables, matched as a
         * case-insensitive substring by the Rust side.
         */
        val defaultDistractingApps = listOf(
            "bilibili",
            "youtube",
            "tiktok",
            "douyin",
            "netflix",
            "steam",
            "epicgames",
            "discord",
            "twitter",
            "instagram",
            "reddit",
            "zhihu",
            "weibo",
        )
    }
}

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
    val accountability: Accountability = Accountability(),
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
    const val NAG = "nag"
    const val CHEER = "cheer"

    /** Ordered with the accountability actions first, as on the desktop. */
    val all = listOf(
        Triple(NAG, "👀", "别摸鱼了"),
        Triple(FOCUS_TOGETHER, "🍅", "一起专注"),
        Triple(CHEER, "🎉", "夸一夸"),
        Triple(KNOCK, "👋", "敲一敲"),
        Triple(HUG, "🤗", "抱抱"),
        Triple(COFFEE, "☕", "请喝咖啡"),
        Triple(REST, "🛋", "去休息"),
    )
}
