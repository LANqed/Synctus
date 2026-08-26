package dev.synctus.app

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Checkbox
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Slider
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp

/**
 * The settings sheet.
 *
 * Edits a local copy and only calls [onSave] when the user confirms, so a
 * half-typed pairing code never reaches the engine and triggers a reconnect.
 */
@Composable
fun SettingsDialog(
    initial: ClientConfig,
    onDismiss: () -> Unit,
    onSave: (ClientConfig) -> Unit,
) {
    var draft by remember { mutableStateOf(initial) }

    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("设置") },
        confirmButton = {
            TextButton(
                onClick = { onSave(draft) },
                enabled = draft.isPaired() && draft.server.isNotBlank(),
            ) {
                Text("保存并应用")
            }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) { Text("取消") }
        },
        text = {
            Column(
                Modifier
                    .heightIn(max = 460.dp)
                    .verticalScroll(rememberScrollState()),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Text("配对", style = MaterialTheme.typography.titleSmall)
                Text(
                    "双方填入同一个配对码。服务器只转发密文，无法读取内容。",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Row(verticalAlignment = Alignment.CenterVertically) {
                    OutlinedTextField(
                        value = draft.inviteCode,
                        onValueChange = { draft = draft.copy(inviteCode = it) },
                        label = { Text("配对码") },
                        singleLine = true,
                        isError = draft.inviteCode.isNotEmpty() && !draft.isPaired(),
                        modifier = Modifier.weight(1f),
                    )
                    Spacer(Modifier.width(8.dp))
                    TextButton(onClick = {
                        val code = NativeBridge.newInviteCode()
                        if (code.isNotEmpty()) draft = draft.copy(inviteCode = code)
                    }) {
                        Text("生成")
                    }
                }

                HorizontalDivider()
                Text("服务器", style = MaterialTheme.typography.titleSmall)
                OutlinedTextField(
                    value = draft.server,
                    onValueChange = { draft = draft.copy(server = it) },
                    label = { Text("地址 host:port") },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
                SwitchRow(
                    label = "使用 TLS（推荐）",
                    checked = draft.tls,
                    onChange = { draft = draft.copy(tls = it) },
                )
                if (!draft.tls) {
                    Text(
                        "关闭 TLS 后房间与设备标识会以明文经过网络，消息内容仍为端到端加密。",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.error,
                    )
                }

                HorizontalDivider()
                Text("本机", style = MaterialTheme.typography.titleSmall)
                OutlinedTextField(
                    value = draft.deviceName,
                    onValueChange = { draft = draft.copy(deviceName = it) },
                    label = { Text("显示名称") },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
                OutlinedTextField(
                    value = draft.user,
                    onValueChange = { draft = draft.copy(user = it) },
                    label = { Text("用户标识（例如 A）") },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
                Text(
                    "标识是分组依据：所有填同一个标识的设备会被归到同一个用户下，服务器管理面板按它分组。",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                SliderRow(
                    label = "采样间隔",
                    value = draft.pollSecs.toFloat(),
                    range = 5f..120f,
                    suffix = " 秒",
                ) { draft = draft.copy(pollSecs = it.toLong()) }
                Text(
                    "间隔越长越省电；状态变化仍会在下一次采样时同步。",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )

                HorizontalDivider()
                Text("督促", style = MaterialTheme.typography.titleSmall)
                Text(
                    "这一栏决定了这个工具有多“烦人”。",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )

                SliderRow(
                    label = "每日目标",
                    value = draft.accountability.dailyGoalMin.toFloat(),
                    range = 0f..600f,
                    suffix = " 分钟",
                ) {
                    draft = draft.copy(
                        accountability = draft.accountability.copy(dailyGoalMin = it.toInt())
                    )
                }
                Text(
                    if (draft.accountability.dailyGoalMin == 0) {
                        "目标为 0：不统计目标与连续天数，只同步状态。"
                    } else {
                        "约 ${
                            Math.ceil(
                                draft.accountability.dailyGoalMin.toDouble() /
                                    draft.pomodoro.focusMin.coerceAtLeast(1)
                            ).toInt()
                        } 个 ${draft.pomodoro.focusMin} 分钟的专注回合。达标后会自动告知对方。"
                    },
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )

                SwitchRow(
                    label = "专注时打开摸鱼应用就提醒我",
                    checked = draft.accountability.warnOnDistraction,
                ) {
                    draft = draft.copy(
                        accountability = draft.accountability.copy(warnOnDistraction = it)
                    )
                }
                Text(
                    "只在专注回合进行中检查；休息和空闲时间不监控。",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )

                if (draft.accountability.warnOnDistraction) {
                    SliderRow(
                        label = "宽限时间",
                        value = draft.accountability.distractionGraceSecs.toFloat(),
                        range = 0f..300f,
                        suffix = " 秒",
                    ) {
                        draft = draft.copy(
                            accountability =
                                draft.accountability.copy(distractionGraceSecs = it.toInt())
                        )
                    }
                    Text(
                        "切过去查资料不算摸鱼；只有停留超过这个时间才提醒。",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )

                    SwitchRow(
                        label = "顺便告诉对方",
                        checked = draft.accountability.reportDistractionToPeer,
                    ) {
                        draft = draft.copy(
                            accountability =
                                draft.accountability.copy(reportDistractionToPeer = it)
                        )
                    }
                    Text(
                        if (draft.accountability.reportDistractionToPeer) {
                            "对方会收到你在专注时摸鱼的提醒。被盯着是自己选的。"
                        } else {
                            "提醒只出现在本机，对方不会知道。"
                        },
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )

                    OutlinedTextField(
                        value = draft.accountability.distractingApps.joinToString("\n"),
                        onValueChange = { text ->
                            draft = draft.copy(
                                accountability = draft.accountability.copy(
                                    distractingApps = text.lines()
                                        .map { it.trim() }
                                        .filter { it.isNotEmpty() },
                                )
                            )
                        },
                        label = { Text("摸鱼应用清单（每行一个，匹配包名的一部分）") },
                        minLines = 3,
                        maxLines = 6,
                        modifier = Modifier.fillMaxWidth(),
                    )
                }

                SwitchRow(
                    label = "允许对方的「别摸鱼了」穿透免打扰",
                    checked = draft.accountability.allowUrgentNudges,
                ) {
                    draft = draft.copy(
                        accountability = draft.accountability.copy(allowUrgentNudges = it)
                    )
                }
                SwitchRow(
                    label = "对方达成目标时自动祝贺",
                    checked = draft.accountability.autoCheer,
                ) {
                    draft = draft.copy(
                        accountability = draft.accountability.copy(autoCheer = it)
                    )
                }

                HorizontalDivider()
                Text("隐私", style = MaterialTheme.typography.titleSmall)
                Text(
                    "关闭的项目根本不会离开本机。",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                SwitchRow(
                    label = "同步前台应用",
                    checked = draft.privacy.shareForegroundApp,
                ) {
                    draft = draft.copy(privacy = draft.privacy.copy(shareForegroundApp = it))
                }
                SwitchRow(
                    label = "同步电量",
                    checked = draft.privacy.shareBattery,
                ) {
                    draft = draft.copy(privacy = draft.privacy.copy(shareBattery = it))
                }
                SwitchRow(
                    label = "同步正在播放的音乐",
                    checked = draft.privacy.shareMusic,
                ) {
                    draft = draft.copy(privacy = draft.privacy.copy(shareMusic = it))
                }
                SwitchRow(
                    label = "同步番茄钟状态",
                    checked = draft.privacy.sharePomodoro,
                ) {
                    draft = draft.copy(privacy = draft.privacy.copy(sharePomodoro = it))
                }
                Text(
                    "同时包含今日专注分钟数、每日目标与连续天数——督促功能依赖这一项。",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                SwitchRow(
                    label = "同步待办清单",
                    checked = draft.privacy.shareTodos,
                ) {
                    draft = draft.copy(privacy = draft.privacy.copy(shareTodos = it))
                }

                OutlinedTextField(
                    value = draft.privacy.blockedApps.joinToString("\n"),
                    onValueChange = { text ->
                        draft = draft.copy(
                            privacy = draft.privacy.copy(
                                blockedApps = text.lines()
                                    .map { it.trim() }
                                    .filter { it.isNotEmpty() },
                            )
                        )
                    },
                    label = { Text("应用黑名单（每行一个包名）") },
                    minLines = 2,
                    maxLines = 4,
                    modifier = Modifier.fillMaxWidth(),
                )

                HorizontalDivider()
                Text("番茄钟", style = MaterialTheme.typography.titleSmall)
                SliderRow(
                    label = "专注时长",
                    value = draft.pomodoro.focusMin.toFloat(),
                    range = 5f..90f,
                    suffix = " 分",
                ) { draft = draft.copy(pomodoro = draft.pomodoro.copy(focusMin = it.toInt())) }
                SliderRow(
                    label = "小休时长",
                    value = draft.pomodoro.shortBreakMin.toFloat(),
                    range = 1f..30f,
                    suffix = " 分",
                ) {
                    draft = draft.copy(pomodoro = draft.pomodoro.copy(shortBreakMin = it.toInt()))
                }
                SliderRow(
                    label = "长休时长",
                    value = draft.pomodoro.longBreakMin.toFloat(),
                    range = 5f..60f,
                    suffix = " 分",
                ) {
                    draft = draft.copy(pomodoro = draft.pomodoro.copy(longBreakMin = it.toInt()))
                }
                SwitchRow(
                    label = "阶段结束后自动继续",
                    checked = draft.pomodoro.autoContinue,
                ) {
                    draft = draft.copy(pomodoro = draft.pomodoro.copy(autoContinue = it))
                }
                SwitchRow(
                    label = "根据阶段自动切换状态",
                    checked = draft.pomodoro.presenceFollowsPhase,
                ) {
                    draft = draft.copy(
                        pomodoro = draft.pomodoro.copy(presenceFollowsPhase = it)
                    )
                }

                HorizontalDivider()
                Text("其他", style = MaterialTheme.typography.titleSmall)
                SwitchRow(
                    label = "开机自动启动",
                    checked = draft.autostart,
                ) { draft = draft.copy(autostart = it) }
                SwitchRow(
                    label = "静音互动提醒",
                    checked = draft.muteNudges,
                ) { draft = draft.copy(muteNudges = it) }
                SwitchRow(
                    label = "检查更新",
                    checked = draft.checkUpdates,
                ) { draft = draft.copy(checkUpdates = it) }
            }
        },
    )
}

@Composable
private fun SwitchRow(
    label: String,
    checked: Boolean,
    onChange: (Boolean) -> Unit,
) {
    Row(
        Modifier.fillMaxWidth(),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(label, Modifier.weight(1f), style = MaterialTheme.typography.bodyMedium)
        Switch(checked = checked, onCheckedChange = onChange)
    }
}

@Composable
private fun SliderRow(
    label: String,
    value: Float,
    range: ClosedFloatingPointRange<Float>,
    suffix: String,
    onChange: (Float) -> Unit,
) {
    Column(Modifier.fillMaxWidth()) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(label, Modifier.weight(1f), style = MaterialTheme.typography.bodyMedium)
            Text(
                "${value.toInt()}$suffix",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        Slider(
            value = value,
            onValueChange = onChange,
            valueRange = range,
        )
    }
}
