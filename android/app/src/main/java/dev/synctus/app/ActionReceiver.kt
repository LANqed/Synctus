package dev.synctus.app

import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/**
 * Handles the notification action buttons.
 *
 * A receiver rather than a service so a tap does nothing more than deliver one
 * command to the already-running engine — no new process, no cold start.
 */
class ActionReceiver : BroadcastReceiver() {

    override fun onReceive(context: Context, intent: Intent) {
        val service = SyncService.instance
        if (service == null) {
            // The service died between showing the notification and the tap. Start it
            // and drop this action: replaying it blind could poke the peer twice.
            SyncService.start(context)
            return
        }

        when (intent.action) {
            ACTION_KNOCK -> service.sendCommand(BridgeCommand.Nudge(NudgeKey.KNOCK))

            ACTION_TOGGLE_REST -> {
                // Reads the current label rather than tracking state here, so the
                // button always does the opposite of what is shown.
                val resting = SyncService.state.value.local.presence == "休息中"
                val next = if (resting) PresenceKey.ACTIVE else PresenceKey.RESTING
                service.sendCommand(BridgeCommand.SetPresence(next))
            }

            ACTION_TOGGLE_POMODORO ->
                service.sendCommand(BridgeCommand.TogglePomodoro)

            // Answering a nag by actually starting a round, straight from the shade.
            ACTION_START_FOCUS ->
                service.sendCommand(BridgeCommand.StartFocus)

            ACTION_NUDGE -> {
                val kind = intent.getStringExtra(EXTRA_KIND) ?: NudgeKey.KNOCK
                // The engine composes the nag text from the peer's own numbers, so
                // no text is passed here.
                service.sendCommand(BridgeCommand.Nudge(kind))
            }
        }

        // Reflect the change immediately instead of waiting for the next poll tick.
        service.refreshNow()
    }

    companion object {
        const val ACTION_KNOCK = "dev.synctus.app.KNOCK"
        const val ACTION_TOGGLE_REST = "dev.synctus.app.TOGGLE_REST"
        const val ACTION_TOGGLE_POMODORO = "dev.synctus.app.TOGGLE_POMODORO"
        const val ACTION_START_FOCUS = "dev.synctus.app.START_FOCUS"
        const val ACTION_NUDGE = "dev.synctus.app.NUDGE"

        private const val EXTRA_KIND = "kind"

        /** Build a broadcast intent for a notification action. */
        fun pendingIntent(context: Context, action: String): PendingIntent {
            val intent = Intent(context, ActionReceiver::class.java).setAction(action)
            return PendingIntent.getBroadcast(
                context,
                action.hashCode(),
                intent,
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
        }

        /** Variant that carries a specific interaction kind. */
        fun nudgeIntent(context: Context, kind: String): PendingIntent {
            val intent = Intent(context, ActionReceiver::class.java)
                .setAction(ACTION_NUDGE)
                .putExtra(EXTRA_KIND, kind)
            return PendingIntent.getBroadcast(
                context,
                (ACTION_NUDGE + kind).hashCode(),
                intent,
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
        }
    }
}
