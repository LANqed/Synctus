package dev.synctus.app

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/**
 * Restarts the sync service after a reboot or an app update.
 *
 * Only acts when the user enabled autostart and a pairing code exists — starting a
 * foreground service that immediately reports "not paired" would be a pointless
 * notification on every boot.
 */
class BootReceiver : BroadcastReceiver() {

    override fun onReceive(context: Context, intent: Intent) {
        val action = intent.action
        if (action != Intent.ACTION_BOOT_COMPLETED &&
            action != Intent.ACTION_MY_PACKAGE_REPLACED
        ) {
            return
        }

        val store = Store(context)
        if (!store.autostart) return
        if (!store.loadConfig().isPaired()) return

        SyncService.start(context)
    }
}
