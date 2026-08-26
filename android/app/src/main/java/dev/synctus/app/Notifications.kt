package dev.synctus.app

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.graphics.Color
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat

/**
 * The notification layer.
 *
 * Two channels with different behaviour:
 *
 * * [CHANNEL_STATUS] — the ongoing foreground notification. Silent and low
 *   importance, because it is on screen permanently; this is also what keeps the
 *   process alive.
 * * [CHANNEL_NUDGE] — pokes from the peer and pomodoro boundaries. High
 *   importance so they actually surface.
 */
object Notifications {

    const val CHANNEL_STATUS = "synctus.status"
    const val CHANNEL_NUDGE = "synctus.nudge"

    const val ID_STATUS = 1001
    private const val ID_NUDGE = 1002
    private const val ID_POMODORO = 1003
    private const val ID_GOAL = 1004

    /** Create both channels. Idempotent, so it runs on every app start. */
    fun createChannels(context: Context) {
        val manager = context.getSystemService(NotificationManager::class.java) ?: return

        val status = NotificationChannel(
            CHANNEL_STATUS,
            context.getString(R.string.channel_status),
            // LOW: visible and persistent, but never makes a sound.
            NotificationManager.IMPORTANCE_LOW,
        ).apply {
            description = context.getString(R.string.channel_status_desc)
            setShowBadge(false)
            enableVibration(false)
            setSound(null, null)
        }

        val nudge = NotificationChannel(
            CHANNEL_NUDGE,
            context.getString(R.string.channel_nudge),
            NotificationManager.IMPORTANCE_HIGH,
        ).apply {
            description = context.getString(R.string.channel_nudge_desc)
            enableVibration(true)
            enableLights(true)
            lightColor = Color.parseColor("#42A5F5")
        }

        manager.createNotificationChannel(status)
        manager.createNotificationChannel(nudge)
    }

    /**
     * Build the ongoing status notification.
     *
     * This is both the UI and the keep-alive mechanism: Android will not kill a
     * process with a visible foreground service notification, which is exactly the
     * "通知栏保活" requirement.
     *
     * The body leads with the two focus numbers, because that comparison is what
     * actually gets someone back to work.
     */
    fun buildStatus(
        context: Context,
        peer: BridgeEvent.Peer?,
        local: LocalStatus,
        connectionState: String,
    ): Notification {
        val title = when {
            peer == null && connectionState == "online" ->
                context.getString(R.string.status_waiting_peer)
            peer == null -> connectionLabel(context, connectionState)
            else -> "${peer.name} · ${peer.presence}"
        }

        // The headline: today's minutes, mine against theirs.
        val comparison = if (local.goalMin > 0) {
            context.getString(
                R.string.status_focus_comparison,
                local.focusTodayMin,
                local.peerFocusTodayMin,
                local.goalMin,
            )
        } else {
            context.getString(
                R.string.status_focus_comparison_no_goal,
                local.focusTodayMin,
                local.peerFocusTodayMin,
            )
        }

        val peerLine = if (peer != null) {
            buildString {
                append(peer.detail)
                if (peer.meta.isNotEmpty()) {
                    append('\n')
                    append(peer.meta)
                }
                if (peer.slacking) {
                    append('\n')
                    append(context.getString(R.string.status_peer_slacking))
                }
            }
        } else {
            context.getString(R.string.status_no_peer_detail)
        }

        // Our own line, so the user can see what the peer is being told.
        val selfLine = buildString {
            append(context.getString(R.string.status_self_prefix))
            append(local.presence)
            if (local.pomodoroActive) {
                append("  🍅")
                append(if (local.pomodoroPaused) "⏸" else "▶")
                append(local.pomodoroPhase)
                append(' ')
                append(local.pomodoroRemaining)
            } else if (local.completedToday > 0) {
                append("  🍅×${local.completedToday}")
            }
            if (local.streakDays > 1) {
                append("  🔥${local.streakDays}")
            }
            if (local.distracted && local.distractedBy != null) {
                append('\n')
                append(context.getString(R.string.status_self_distracted, local.distractedBy))
            }
        }

        val builder = NotificationCompat.Builder(context, CHANNEL_STATUS)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(title)
            // The collapsed line is the comparison: it is the one thing worth
            // seeing without expanding.
            .setContentText(comparison)
            .setStyle(
                NotificationCompat.BigTextStyle()
                    .bigText("$comparison\n$peerLine\n$selfLine")
            )
            .setContentIntent(openAppIntent(context))
            .setOngoing(true)
            .setSilent(true)
            .setShowWhen(false)
            .setOnlyAlertOnce(true)
            .setCategory(NotificationCompat.CATEGORY_STATUS)
            .setPriority(NotificationCompat.PRIORITY_LOW)
            .setVisibility(NotificationCompat.VISIBILITY_PUBLIC)

        // A progress bar for the daily goal. Determinate and unobtrusive; it makes
        // "how far along am I" answerable at a glance.
        if (local.goalMin > 0) {
            builder.setProgress(local.goalMin, local.focusTodayMin.coerceAtMost(local.goalMin), false)
        }

        if (peer != null) {
            builder.color = peer.presenceColor.toInt()
            builder.setColorized(false)
        }

        // Action 1: nag when they are slacking, knock otherwise. One button that
        // always does the most useful thing beats two that need reading.
        if (peer?.slacking == true) {
            builder.addAction(
                NotificationCompat.Action(
                    R.drawable.ic_knock,
                    context.getString(R.string.action_nag),
                    ActionReceiver.nudgeIntent(context, NudgeKey.NAG),
                )
            )
        } else if (peer?.focusing == true) {
            builder.addAction(
                NotificationCompat.Action(
                    R.drawable.ic_knock,
                    context.getString(R.string.action_knock),
                    ActionReceiver.pendingIntent(context, ActionReceiver.ACTION_KNOCK),
                )
            )
        } else {
            // They are not focusing, so invite them instead of poking.
            builder.addAction(
                NotificationCompat.Action(
                    R.drawable.ic_pomodoro,
                    context.getString(R.string.action_focus_together),
                    ActionReceiver.nudgeIntent(context, NudgeKey.FOCUS_TOGETHER),
                )
            )
        }

        // Action 2: the pomodoro, which is the thing the user acts on most.
        builder.addAction(
            NotificationCompat.Action(
                R.drawable.ic_pomodoro,
                context.getString(
                    when {
                        !local.pomodoroActive -> R.string.action_start_focus
                        local.pomodoroPaused -> R.string.action_resume
                        else -> R.string.action_pause
                    }
                ),
                ActionReceiver.pendingIntent(context, ActionReceiver.ACTION_TOGGLE_POMODORO),
            )
        )

        // Action 3: toggle rest, which is how presence is set from the shade.
        val restingNow = local.presence == "休息中"
        builder.addAction(
            NotificationCompat.Action(
                R.drawable.ic_rest,
                context.getString(
                    if (restingNow) R.string.action_back_to_work else R.string.action_rest
                ),
                ActionReceiver.pendingIntent(context, ActionReceiver.ACTION_TOGGLE_REST),
            )
        )

        return builder.build()
    }

    /** Refresh the ongoing notification in place. */
    fun updateStatus(context: Context, notification: Notification) {
        // POST_NOTIFICATIONS can be revoked while the service runs; the service's
        // own notification survives, but an explicit notify would throw.
        try {
            NotificationManagerCompat.from(context).notify(ID_STATUS, notification)
        } catch (e: SecurityException) {
            // Nothing to do: the foreground notification stays as it was.
        }
    }

    /** Show an incoming poke. */
    fun showNudge(context: Context, event: BridgeEvent.Nudge) {
        val builder = NotificationCompat.Builder(context, CHANNEL_NUDGE)
            .setSmallIcon(R.drawable.ic_knock)
            .setContentTitle(event.title)
            .setContentText(event.body)
            .setStyle(NotificationCompat.BigTextStyle().bigText(event.body))
            .setContentIntent(openAppIntent(context))
            .setAutoCancel(true)
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .setCategory(NotificationCompat.CATEGORY_SOCIAL)
            .setDefaults(NotificationCompat.DEFAULT_ALL)

        // An urgent nudge is the one thing allowed to interrupt: a nag that waits
        // for the user to glance at their phone does nothing.
        if (event.urgent) {
            builder
                .setCategory(NotificationCompat.CATEGORY_ALARM)
                // Heads-up display even when a full-screen app is in front.
                .setFullScreenIntent(openAppIntent(context), false)
        }

        // A distraction warning is about the user's own behaviour, so poking back
        // makes no sense; the peer's pokes get a reply button.
        if (event.kind != "distraction") {
            builder.addAction(
                NotificationCompat.Action(
                    R.drawable.ic_knock,
                    context.getString(R.string.action_knock_back),
                    ActionReceiver.pendingIntent(context, ActionReceiver.ACTION_KNOCK),
                )
            )
            // Answering a nag by actually starting a round is the useful reply.
            builder.addAction(
                NotificationCompat.Action(
                    R.drawable.ic_pomodoro,
                    context.getString(R.string.action_start_focus),
                    ActionReceiver.pendingIntent(context, ActionReceiver.ACTION_START_FOCUS),
                )
            )
        }

        try {
            NotificationManagerCompat.from(context).notify(ID_NUDGE, builder.build())
        } catch (e: SecurityException) {
            // Notifications not granted; the in-app UI still shows it.
        }
    }

    /** Announce a pomodoro boundary. */
    fun showPomodoro(context: Context, event: BridgeEvent.Pomodoro) {
        if (!event.finished) return

        val notification = NotificationCompat.Builder(context, CHANNEL_NUDGE)
            .setSmallIcon(R.drawable.ic_pomodoro)
            .setContentTitle(context.getString(R.string.pomodoro_title))
            .setContentText(event.message)
            .setContentIntent(openAppIntent(context))
            .setAutoCancel(true)
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .setCategory(NotificationCompat.CATEGORY_ALARM)
            .setDefaults(NotificationCompat.DEFAULT_ALL)
            .build()

        try {
            NotificationManagerCompat.from(context).notify(ID_POMODORO, notification)
        } catch (e: SecurityException) {
            // As above.
        }
    }

    /**
     * Celebrate meeting the daily goal.
     *
     * Separate from [showPomodoro] so it does not get lost among round-finished
     * messages: hitting the target is the moment worth noticing.
     */
    fun showGoalReached(context: Context, goalMin: Int, streakDays: Int) {
        val body = if (streakDays > 1) {
            context.getString(R.string.goal_body_streak, goalMin, streakDays)
        } else {
            context.getString(R.string.goal_body, goalMin)
        }

        val notification = NotificationCompat.Builder(context, CHANNEL_NUDGE)
            .setSmallIcon(R.drawable.ic_pomodoro)
            .setContentTitle(context.getString(R.string.goal_title))
            .setContentText(body)
            .setContentIntent(openAppIntent(context))
            .setAutoCancel(true)
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .setCategory(NotificationCompat.CATEGORY_SOCIAL)
            .setDefaults(NotificationCompat.DEFAULT_ALL)
            .build()

        try {
            NotificationManagerCompat.from(context).notify(ID_GOAL, notification)
        } catch (e: SecurityException) {
            // As above.
        }
    }

    private fun connectionLabel(context: Context, state: String): String =
        context.getString(
            when (state) {
                "online" -> R.string.conn_online
                "connecting" -> R.string.conn_connecting
                "rejected" -> R.string.conn_rejected
                else -> R.string.conn_offline
            }
        )

    private fun openAppIntent(context: Context): PendingIntent {
        val intent = Intent(context, MainActivity::class.java).apply {
            flags = Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP
        }
        return PendingIntent.getActivity(
            context,
            0,
            intent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
    }
}
