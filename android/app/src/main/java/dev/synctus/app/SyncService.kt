package dev.synctus.app

import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import androidx.core.app.ServiceCompat
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.serialization.encodeToString

/**
 * The always-on half of the app.
 *
 * A foreground service is the only way to keep a socket open on modern Android,
 * and its ongoing notification doubles as the status display — the
 * "通知栏保活" and "通知栏通知显示" requirements are the same mechanism here.
 *
 * The loop is deliberately simple:
 *
 * ```text
 * every tick (default 15s):
 *   read sensors ──▶ NativeBridge.command(publish)
 *   NativeBridge.poll() ──▶ update notification, raise nudge/pomodoro alerts
 * ```
 *
 * State lives in a companion-level flow rather than on the instance, so the
 * Compose UI keeps rendering across a service restart and can read the last known
 * status before the service exists.
 */
class SyncService : Service() {

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private lateinit var store: Store
    private lateinit var sensors: Sensors

    /**
     * Day the peer was last congratulated, so [BridgeEvent.Peer] arriving every
     * poll does not produce a stream of cheers.
     */
    private var cheeredPeerDate: String? = null

    override fun onCreate() {
        super.onCreate()
        store = Store(this)
        sensors = Sensors(this)
        instance = this
        update { it.copy(config = store.loadConfig()) }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        // Promote to foreground first. Android allows only a few seconds before it
        // kills the service with ForegroundServiceDidNotStartInTimeException, so this
        // happens before any engine work.
        startForegroundNotification()

        val config = store.loadConfig()
        update { it.copy(config = config) }

        if (!config.isPaired()) {
            // Nothing to connect to. Stay foreground so the notification can explain
            // why, instead of dying silently.
            update {
                it.copy(
                    connection = STATE_UNPAIRED,
                    warning = getString(R.string.warn_not_paired),
                )
            }
            refreshNotification()
            return START_STICKY
        }

        if (!NativeBridge.isRunning()) {
            val result = NativeBridge.start(SynctusJson.encodeToString(config))
            if (result.isFailure) {
                update {
                    it.copy(
                        connection = STATE_ERROR,
                        warning = result.exceptionOrNull()?.message
                            ?: getString(R.string.warn_engine_failed),
                    )
                }
                refreshNotification()
                return START_STICKY
            }
            // Publish the to-do list once so the peer has it on connect.
            sendCommand(BridgeCommand.SetTodos(store.loadTodos()))

            // The engine starts from zero, so tell it where today left off.
            // Without this a service restart would silently reset the day's
            // progress — exactly the number this whole thing is about.
            val progress = store.loadProgress()
            sendCommand(
                BridgeCommand.RestoreProgress(
                    focusTodayMin = progress.focusTodayMin,
                    streakDays = progress.streakDays,
                )
            )
        }

        if (loopRunning.compareAndSet(false, true)) {
            scope.launch { runLoop(config.pollSecs) }
        }

        // START_STICKY: after a low-memory kill Android restarts us with a null
        // intent, which the checks above handle.
        return START_STICKY
    }

    private suspend fun runLoop(pollSecs: Long) {
        val intervalMs = pollSecs.coerceIn(5, 120) * 1000

        while (true) {
            publishSensors()
            drainEvents()
            refreshNotification()
            delay(intervalMs)
        }
    }

    /** Read what Android will tell us and hand it to the engine. */
    private fun publishSensors() {
        val privacy = state.value.config?.privacy ?: Privacy()

        sendCommand(
            BridgeCommand.Publish(
                foreground = if (privacy.shareForegroundApp) sensors.foregroundApp() else null,
                battery = if (privacy.shareBattery) sensors.battery() else null,
                music = if (privacy.shareMusic) sensors.nowPlaying() else null,
            )
        )
    }

    /** Pull events out of the engine and react to them. */
    private fun drainEvents() {
        val json = NativeBridge.poll()
        val events = try {
            SynctusJson.decodeFromString<List<BridgeEvent>>(json)
        } catch (e: Exception) {
            // A decode failure means Rust and Kotlin disagree about the protocol.
            // Surface it rather than silently dropping status updates.
            update { it.copy(warning = "事件解析失败: ${e.message}") }
            emptyList()
        }

        for (event in events) {
            when (event) {
                is BridgeEvent.Connection -> update {
                    it.copy(
                        connection = event.state,
                        warning = event.detail.takeIf { d -> d.isNotEmpty() },
                    )
                }

                is BridgeEvent.Peer -> {
                    // Congratulate once when they reach their goal. Encouragement
                    // that depends on someone remembering to send it does not
                    // happen, so it is automatic.
                    val autoCheer = state.value.config?.accountability?.autoCheer ?: true
                    if (autoCheer && event.goalMet() && !cheeredPeerToday()) {
                        markPeerCheered()
                        sendCommand(
                            BridgeCommand.Nudge(
                                kind = NudgeKey.CHEER,
                                text = "今天 ${event.focusTodayMin} 分钟，达标了！",
                            )
                        )
                    }
                    update { it.copy(peer = event) }
                }

                is BridgeEvent.Nudge -> {
                    if (state.value.config?.muteNudges != true) {
                        Notifications.showNudge(this, event)
                    }
                    update { it.copy(lastNudge = event, lastNudgeAt = System.currentTimeMillis()) }
                }

                is BridgeEvent.Pomodoro -> {
                    Notifications.showPomodoro(this, event)
                    // The engine keeps the totals in memory only, so persist them
                    // after every boundary: the process can be killed at any time.
                    persistProgress()
                }

                is BridgeEvent.GoalReached -> {
                    // The store decides the streak, so its answer wins over the
                    // engine's in-memory guess.
                    val streak = store.registerGoalMet()
                    Notifications.showGoalReached(this, event.goalMin, streak)
                    persistProgress()
                }

                is BridgeEvent.PeerTodos -> update { it.copy(peerTodos = event.items) }

                is BridgeEvent.Warning -> update { it.copy(warning = event.message) }
            }
        }

        // Local status is polled, not pushed: it changes every second during a
        // pomodoro and an event per second would be wasteful.
        val local = readLocalStatus()
        update { it.copy(local = local) }
    }

    private fun readLocalStatus(): LocalStatus = try {
        SynctusJson.decodeFromString<LocalStatus>(NativeBridge.localStatus())
    } catch (e: Exception) {
        LocalStatus()
    }

    /**
     * Write the engine's focus totals to disk.
     *
     * Called after every pomodoro boundary rather than on a timer: the process can
     * be killed at any moment, and losing today's minutes is the one piece of state
     * that would actually annoy someone.
     */
    private fun persistProgress() {
        val local = readLocalStatus()
        store.saveProgress(local.focusTodayMin, local.streakDays)
        update { it.copy(local = local) }
    }

    /** Whether the peer was already congratulated today. */
    private fun cheeredPeerToday(): Boolean = cheeredPeerDate == todayKey()

    private fun markPeerCheered() {
        cheeredPeerDate = todayKey()
    }

    private fun todayKey(): String = (System.currentTimeMillis() / 86_400_000L).toString()

    private fun startForegroundNotification() {
        val notification = buildNotification()

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            ServiceCompat.startForeground(
                this,
                Notifications.ID_STATUS,
                notification,
                ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC,
            )
        } else {
            startForeground(Notifications.ID_STATUS, notification)
        }
    }

    private fun buildNotification() = Notifications.buildStatus(
        this,
        peer = state.value.peer,
        local = state.value.local,
        connectionState = state.value.connection,
    )

    private fun refreshNotification() {
        Notifications.updateStatus(this, buildNotification())
    }

    /** Send a command, recording any failure for the UI. */
    fun sendCommand(command: BridgeCommand) {
        val json = SynctusJson.encodeToString(command)
        NativeBridge.command(json).onFailure { error ->
            update { it.copy(warning = error.message) }
        }
    }

    /**
     * Apply an edited config: persist it, reconfigure the engine, refresh the
     * notification.
     */
    fun applyConfig(config: ClientConfig) {
        store.saveConfig(config)
        store.autostart = config.autostart
        update { it.copy(config = config) }

        if (NativeBridge.isRunning()) {
            sendCommand(BridgeCommand.Reconfigure(config))
        } else if (config.isPaired()) {
            // Was unpaired until now; start the engine.
            NativeBridge.start(SynctusJson.encodeToString(config)).onSuccess {
                sendCommand(BridgeCommand.SetTodos(store.loadTodos()))
                val progress = store.loadProgress()
                sendCommand(
                    BridgeCommand.RestoreProgress(
                        focusTodayMin = progress.focusTodayMin,
                        streakDays = progress.streakDays,
                    )
                )
                if (loopRunning.compareAndSet(false, true)) {
                    scope.launch { runLoop(config.pollSecs) }
                }
            }
        }
        refreshNotification()
    }

    fun saveTodos(todos: List<Todo>) {
        store.saveTodos(todos)
        sendCommand(BridgeCommand.SetTodos(todos))
    }

    fun loadTodos(): List<Todo> = store.loadTodos()

    /** Force an immediate sample-and-poll, used when the UI comes to the front. */
    fun refreshNow() {
        scope.launch {
            publishSensors()
            drainEvents()
            refreshNotification()
        }
    }

    override fun onDestroy() {
        // Save before tearing down: whatever minutes were earned since the last
        // boundary would otherwise be lost.
        persistProgress()
        scope.cancel()
        loopRunning.set(false)
        NativeBridge.stop()
        instance = null
        super.onDestroy()
    }

    /**
     * Restart when the task is swiped away, if the user wants to stay connected.
     *
     * A foreground service survives memory pressure, but swiping the task away
     * still stops it on many vendor ROMs; this is the other half of keep-alive.
     */
    override fun onTaskRemoved(rootIntent: Intent?) {
        if (store.autostart) {
            startService(Intent(applicationContext, SyncService::class.java))
        }
        super.onTaskRemoved(rootIntent)
    }

    override fun onBind(intent: Intent?): IBinder? = null

    /** Everything the UI needs, in one snapshot. */
    data class ServiceState(
        val connection: String = STATE_CONNECTING,
        val peer: BridgeEvent.Peer? = null,
        val local: LocalStatus = LocalStatus(),
        val peerTodos: List<Todo> = emptyList(),
        val lastNudge: BridgeEvent.Nudge? = null,
        val lastNudgeAt: Long = 0,
        val warning: String? = null,
        val config: ClientConfig? = null,
    )

    companion object {
        const val STATE_CONNECTING = "connecting"
        const val STATE_ONLINE = "online"
        const val STATE_OFFLINE = "offline"
        const val STATE_REJECTED = "rejected"
        const val STATE_UNPAIRED = "unpaired"
        const val STATE_ERROR = "error"

        /**
         * The running service, if any.
         *
         * A plain reference rather than a binder: the activity and the service share
         * a process, so binding would add ceremony without adding safety. Cleared in
         * [onDestroy]; every caller null-checks.
         */
        @Volatile
        var instance: SyncService? = null
            private set

        private val loopRunning = java.util.concurrent.atomic.AtomicBoolean(false)

        private val mutableState = MutableStateFlow(ServiceState())

        /** Observable state, readable before the service exists. */
        val state: StateFlow<ServiceState> = mutableState.asStateFlow()

        private fun update(transform: (ServiceState) -> ServiceState) {
            mutableState.value = transform(mutableState.value)
        }

        fun start(context: Context) {
            val intent = Intent(context, SyncService::class.java)
            // startForegroundService is required from O onwards; the service then has
            // a few seconds to call startForeground.
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                context.startForegroundService(intent)
            } else {
                context.startService(intent)
            }
        }

        fun stop(context: Context) {
            context.stopService(Intent(context, SyncService::class.java))
        }
    }
}
