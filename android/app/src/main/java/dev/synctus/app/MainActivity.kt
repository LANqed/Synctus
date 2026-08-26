package dev.synctus.app

import android.Manifest
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AssistChip
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.Checkbox
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.content.ContextCompat
import androidx.lifecycle.compose.collectAsStateWithLifecycle

/**
 * The single screen of the Android app.
 *
 * Everything long-running lives in [SyncService]; this activity only renders
 * [SyncService.state] and sends commands. That is what makes closing the app
 * harmless — the sync keeps running in the foreground service.
 */
class MainActivity : ComponentActivity() {

    private val notificationPermission = registerForActivityResult(
        ActivityResultContracts.RequestPermission()
    ) {
        // Start either way: without the permission the notification is hidden, but
        // the sync itself still works.
        SyncService.start(this)
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        ensureNotificationPermission()

        setContent {
            SynctusTheme {
                MainScreen(
                    onOpenUsageAccess = { startActivity(Sensors.usageAccessIntent()) },
                    onOpenNotificationAccess = {
                        startActivity(Sensors.notificationAccessIntent())
                    },
                    onOpenBatterySettings = {
                        startActivity(Sensors.batterySettingsIntent(this))
                    },
                )
            }
        }
    }

    override fun onResume() {
        super.onResume()
        // Coming back to the front is a good moment to refresh, so the user does not
        // stare at data from the previous poll tick.
        SyncService.instance?.refreshNow()
    }

    /**
     * Ask for notification permission, then start the service.
     *
     * From Android 13 the foreground notification is invisible without it, which
     * would make a working app look broken.
     */
    private fun ensureNotificationPermission() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) {
            SyncService.start(this)
            return
        }

        val granted = ContextCompat.checkSelfPermission(
            this,
            Manifest.permission.POST_NOTIFICATIONS,
        ) == PackageManager.PERMISSION_GRANTED

        if (granted) {
            SyncService.start(this)
        } else {
            notificationPermission.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
    }
}

@Composable
private fun SynctusTheme(content: @Composable () -> Unit) {
    val colours = if (isSystemInDarkTheme()) darkColorScheme() else lightColorScheme()
    MaterialTheme(colorScheme = colours, content = content)
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun MainScreen(
    onOpenUsageAccess: () -> Unit,
    onOpenNotificationAccess: () -> Unit,
    onOpenBatterySettings: () -> Unit,
) {
    val state by SyncService.state.collectAsStateWithLifecycle()
    var showSettings by remember { mutableStateOf(false) }

    // Prompt for pairing on first launch, where nothing else can work yet.
    LaunchedEffect(state.config?.inviteCode) {
        if (state.config != null && state.config?.isPaired() == false) {
            showSettings = true
        }
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("Synctus") },
                actions = {
                    ConnectionBadge(state.connection)
                    Spacer(Modifier.width(8.dp))
                    TextButton(onClick = { showSettings = true }) { Text("设置") }
                },
            )
        }
    ) { padding ->
        Column(
            modifier = Modifier
                .padding(padding)
                .fillMaxSize()
                .verticalScroll(rememberScrollState())
                .padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            if (!NativeBridge.available) {
                Card(
                    Modifier.fillMaxWidth(),
                    colors = CardDefaults.cardColors(
                        containerColor = MaterialTheme.colorScheme.errorContainer
                    ),
                ) {
                    Column(Modifier.padding(12.dp)) {
                        Text("原生库未加载", style = MaterialTheme.typography.titleSmall)
                        Text(
                            NativeBridge.loadError ?: "未知原因",
                            style = MaterialTheme.typography.bodySmall,
                        )
                    }
                }
            }

            PeerCard(state)
            NudgeRow()
            PomodoroCard(state.local)
            PermissionHints(
                onOpenUsageAccess = onOpenUsageAccess,
                onOpenNotificationAccess = onOpenNotificationAccess,
                onOpenBatterySettings = onOpenBatterySettings,
            )
            TodoSection(state)

            state.warning?.let { warning ->
                Text(
                    warning,
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.error,
                )
            }

            Text(
                "引擎版本 ${NativeBridge.version()}",
                style = MaterialTheme.typography.labelSmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }

    if (showSettings) {
        SettingsDialog(
            initial = state.config ?: ClientConfig(),
            onDismiss = { showSettings = false },
            onSave = { config ->
                SyncService.instance?.applyConfig(config)
                showSettings = false
            },
        )
    }
}

@Composable
private fun ConnectionBadge(connection: String) {
    val label: String
    val colour: Color
    when (connection) {
        SyncService.STATE_ONLINE -> {
            label = "已连接"
            colour = Color(0xFF4CAF50)
        }
        SyncService.STATE_CONNECTING -> {
            label = "连接中"
            colour = Color(0xFFFFB300)
        }
        SyncService.STATE_REJECTED -> {
            label = "被拒绝"
            colour = Color(0xFFEF5350)
        }
        SyncService.STATE_UNPAIRED -> {
            label = "未配对"
            colour = Color(0xFF9E9E9E)
        }
        else -> {
            label = "离线"
            colour = Color(0xFF9E9E9E)
        }
    }

    Row(verticalAlignment = Alignment.CenterVertically) {
        Box(
            Modifier
                .size(8.dp)
                .clip(CircleShape)
                .background(colour)
        )
        Spacer(Modifier.width(6.dp))
        Text(label, style = MaterialTheme.typography.labelMedium)
    }
}

@Composable
private fun PeerCard(state: SyncService.ServiceState) {
    val peer = state.peer

    Card(Modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp)) {
            if (peer == null) {
                Text("等待对方…", style = MaterialTheme.typography.titleMedium)
                Spacer(Modifier.height(4.dp))
                Text(
                    "对方上线后，这里会显示 TA 的状态、正在使用的应用、播放的音乐和电量。",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                return@Column
            }

            Row(verticalAlignment = Alignment.CenterVertically) {
                Box(
                    Modifier
                        .size(48.dp)
                        .clip(CircleShape)
                        .background(Color(peer.presenceColor.toInt()).copy(alpha = 0.25f)),
                    contentAlignment = Alignment.Center,
                ) {
                    Text(
                        peer.platform.take(3).uppercase(),
                        fontSize = 11.sp,
                        fontWeight = FontWeight.Bold,
                    )
                }
                Spacer(Modifier.width(12.dp))
                Column(Modifier.weight(1f)) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(peer.name, style = MaterialTheme.typography.titleMedium)
                        Spacer(Modifier.width(8.dp))
                        Text(
                            peer.presence,
                            style = MaterialTheme.typography.labelMedium,
                            color = Color(peer.presenceColor.toInt()),
                        )
                    }
                    Text(
                        peer.detail,
                        style = MaterialTheme.typography.bodyMedium,
                        maxLines = 2,
                        overflow = TextOverflow.Ellipsis,
                    )
                    if (peer.meta.isNotEmpty()) {
                        Text(
                            peer.meta,
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
            }

            // A poke that arrived in the last minute, so it is visible even if the
            // notification was missed.
            val nudge = state.lastNudge
            if (nudge != null && System.currentTimeMillis() - state.lastNudgeAt < 60_000) {
                Spacer(Modifier.height(8.dp))
                HorizontalDivider()
                Spacer(Modifier.height(8.dp))
                Text(nudge.body, style = MaterialTheme.typography.bodyMedium)
            }
        }
    }
}

@Composable
private fun NudgeRow() {
    Card(Modifier.fillMaxWidth()) {
        Column(Modifier.padding(12.dp)) {
            Text("互动", style = MaterialTheme.typography.titleSmall)
            Spacer(Modifier.height(8.dp))
            Row(
                horizontalArrangement = Arrangement.spacedBy(8.dp),
                modifier = Modifier.fillMaxWidth(),
            ) {
                NudgeKey.all.forEach { entry ->
                    val kind = entry.first
                    val emoji = entry.second
                    val label = entry.third
                    AssistChip(
                        onClick = {
                            SyncService.instance?.let { service ->
                                service.sendCommand(BridgeCommand.Nudge(kind))
                                service.refreshNow()
                            }
                        },
                        label = { Text(emoji, fontSize = 16.sp) },
                        modifier = Modifier.semantics { contentDescription = label },
                    )
                }
            }
        }
    }
}

@Composable
private fun PomodoroCard(local: LocalStatus) {
    Card(Modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text(
                        if (local.pomodoroActive) {
                            "${local.pomodoroPhase} ${local.pomodoroRemaining}"
                        } else {
                            "番茄钟"
                        },
                        style = MaterialTheme.typography.titleMedium,
                    )
                    Text(
                        "今日完成 ${local.completedToday} 个回合 · 我的状态：${local.presence}",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }

                Button(onClick = {
                    SyncService.instance?.let { service ->
                        service.sendCommand(BridgeCommand.TogglePomodoro)
                        service.refreshNow()
                    }
                }) {
                    Text(
                        when {
                            !local.pomodoroActive -> "开始"
                            local.pomodoroPaused -> "继续"
                            else -> "暂停"
                        }
                    )
                }

                if (local.pomodoroActive) {
                    Spacer(Modifier.width(8.dp))
                    OutlinedButton(onClick = {
                        SyncService.instance?.let { service ->
                            service.sendCommand(BridgeCommand.StopPomodoro)
                            service.refreshNow()
                        }
                    }) {
                        Text("停止")
                    }
                }
            }

            Spacer(Modifier.height(8.dp))
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                PresenceKey.selectable.forEach { entry ->
                    val key = entry.first
                    val label = entry.second
                    FilterChip(
                        selected = local.presence == label,
                        onClick = {
                            SyncService.instance?.let { service ->
                                service.sendCommand(BridgeCommand.SetPresence(key))
                                service.refreshNow()
                            }
                        },
                        label = { Text(label, fontSize = 12.sp) },
                    )
                }
            }
        }
    }
}

@Composable
private fun PermissionHints(
    onOpenUsageAccess: () -> Unit,
    onOpenNotificationAccess: () -> Unit,
    onOpenBatterySettings: () -> Unit,
) {
    val context = LocalContext.current
    val sensors = remember { Sensors(context) }

    // Recomputed on recomposition, which happens when the user returns from the
    // system settings screen they were sent to.
    val needsUsage = !sensors.hasUsageAccess()
    val needsNotification = !sensors.hasNotificationAccess()
    val needsBattery = !Sensors.isIgnoringBatteryOptimizations(context)

    if (!needsUsage && !needsNotification && !needsBattery) return

    Card(
        Modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(
            containerColor = MaterialTheme.colorScheme.surfaceVariant
        ),
    ) {
        Column(Modifier.padding(12.dp)) {
            Text("可选权限", style = MaterialTheme.typography.titleSmall)
            Spacer(Modifier.height(4.dp))
            Text(
                "不授予也能同步，只是少几项信息。",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Spacer(Modifier.height(8.dp))

            if (needsUsage) {
                TextButton(onClick = onOpenUsageAccess) {
                    Text("允许读取使用情况 → 同步前台应用")
                }
            }
            if (needsNotification) {
                TextButton(onClick = onOpenNotificationAccess) {
                    Text("允许通知访问 → 同步正在播放的音乐")
                }
            }
            if (needsBattery) {
                TextButton(onClick = onOpenBatterySettings) {
                    Text("取消电池优化 → 后台不被杀死")
                }
            }
        }
    }
}

@Composable
private fun TodoSection(state: SyncService.ServiceState) {
    var todos by remember {
        mutableStateOf(SyncService.instance?.loadTodos() ?: emptyList())
    }
    var draft by remember { mutableStateOf("") }

    fun persist(next: List<Todo>) {
        todos = next
        SyncService.instance?.saveTodos(next)
    }

    Card(Modifier.fillMaxWidth()) {
        Column(Modifier.padding(12.dp)) {
            Text("待办", style = MaterialTheme.typography.titleSmall)
            Spacer(Modifier.height(8.dp))

            Row(verticalAlignment = Alignment.CenterVertically) {
                OutlinedTextField(
                    value = draft,
                    onValueChange = { draft = it },
                    placeholder = { Text("添加待办") },
                    singleLine = true,
                    modifier = Modifier.weight(1f),
                )
                Spacer(Modifier.width(8.dp))
                Button(
                    onClick = {
                        val title = draft.trim()
                        if (title.isNotEmpty()) {
                            persist(
                                todos + Todo(
                                    id = java.util.UUID.randomUUID().toString().take(16),
                                    title = title,
                                    createdAt = System.currentTimeMillis(),
                                )
                            )
                            draft = ""
                        }
                    },
                    enabled = draft.isNotBlank(),
                ) {
                    Text("添加")
                }
            }

            todos.forEach { todo ->
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Checkbox(
                        checked = todo.done,
                        onCheckedChange = { checked ->
                            persist(
                                todos.map { item ->
                                    if (item.id == todo.id) {
                                        item.copy(
                                            done = checked,
                                            doneAt = if (checked) {
                                                System.currentTimeMillis()
                                            } else {
                                                null
                                            },
                                        )
                                    } else {
                                        item
                                    }
                                }
                            )
                        },
                    )
                    Text(
                        todo.title,
                        Modifier.weight(1f),
                        style = MaterialTheme.typography.bodyMedium,
                    )
                    TextButton(onClick = { persist(todos.filterNot { it.id == todo.id }) }) {
                        Text("删除")
                    }
                }
            }

            if (todos.any { it.done }) {
                TextButton(onClick = { persist(todos.filterNot { it.done }) }) {
                    Text("清除已完成")
                }
            }

            if (state.peerTodos.isNotEmpty()) {
                Spacer(Modifier.height(8.dp))
                HorizontalDivider()
                Spacer(Modifier.height(8.dp))
                Text(
                    "${state.peer?.name ?: "对方"} 的待办",
                    style = MaterialTheme.typography.titleSmall,
                )
                Column(Modifier.heightIn(max = 160.dp)) {
                    state.peerTodos.forEach { todo ->
                        Text(
                            "${if (todo.done) "☑" else "☐"} ${todo.title}",
                            style = MaterialTheme.typography.bodySmall,
                        )
                    }
                }
            }
        }
    }
}
