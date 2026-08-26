package dev.synctus.app

import android.app.Application

/**
 * Loads the native library and creates the notification channels once per process.
 *
 * Doing it here rather than in the activity means [SyncService] can be started by
 * [BootReceiver] without the UI ever being shown.
 */
class SynctusApp : Application() {

    override fun onCreate() {
        super.onCreate()
        NativeBridge.load()
        Notifications.createChannels(this)
    }
}
