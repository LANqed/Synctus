package dev.synctus.app

import android.app.ActivityManager
import android.app.usage.UsageStatsManager
import android.content.Context
import android.content.Intent
import android.media.session.MediaController
import android.media.session.MediaSessionManager
import android.os.BatteryManager
import android.provider.Settings

/**
 * Reads local state from Android.
 *
 * Every method degrades to `null` when the platform will not tell us, which is the
 * normal case for the foreground app (needs usage-access) and the media session
 * (needs notification-access). The sync works without either.
 */
class Sensors(private val context: Context) {

    /** Battery level and charging state, from the sticky battery broadcast. */
    fun battery(): Battery? {
        val manager = context.getSystemService(BatteryManager::class.java) ?: return null
        val percent = manager.getIntProperty(BatteryManager.BATTERY_PROPERTY_CAPACITY)
        if (percent < 0 || percent > 100) return null

        val charging = manager.isCharging

        // `BATTERY_PROPERTY_CHARGE_COUNTER` style estimates are unreliable across
        // vendors, so only report the OS estimate when it exists.
        val minutesLeft = if (android.os.Build.VERSION.SDK_INT >= 28 && !charging) {
            val nanos = manager.computeChargeTimeRemaining()
            if (nanos > 0) (nanos / 1000 / 60).toInt() else null
        } else {
            null
        }

        return Battery(percent = percent, charging = charging, minutesLeft = minutesLeft)
    }

    /**
     * The app currently in the foreground.
     *
     * Requires usage-access, which the user must grant manually. Returns `null`
     * without it — Android deliberately gives no way to read this otherwise.
     */
    fun foregroundApp(): ForegroundApp? {
        if (!hasUsageAccess()) return null

        val manager = context.getSystemService(UsageStatsManager::class.java) ?: return null
        val now = System.currentTimeMillis()
        // A 10-second window: long enough to catch the current app, short enough
        // that a stale entry does not win.
        val events = manager.queryEvents(now - 10_000, now)

        var lastPackage: String? = null
        val event = android.app.usage.UsageEvents.Event()
        while (events.hasNextEvent()) {
            events.getNextEvent(event)
            if (event.eventType == android.app.usage.UsageEvents.Event.ACTIVITY_RESUMED) {
                lastPackage = event.packageName
            }
        }

        val pkg = lastPackage ?: return null
        // Do not report ourselves; it is noise, and it would flip every time the
        // user opens the app to check on the peer.
        if (pkg == context.packageName) return null

        return ForegroundApp(app = pkg, name = appLabel(pkg), title = null)
    }

    /**
     * The active media session.
     *
     * Reading sessions requires the notification-listener permission, which is
     * granted to [MediaListenerService]; the component name below is what
     * authorises this call.
     */
    fun nowPlaying(): NowPlaying? {
        if (!hasNotificationAccess()) return null

        val manager = context.getSystemService(MediaSessionManager::class.java) ?: return null
        val component = android.content.ComponentName(context, MediaListenerService::class.java)

        val controllers: List<MediaController> = try {
            manager.getActiveSessions(component)
        } catch (e: SecurityException) {
            // Access revoked between the check and the call.
            return null
        }

        for (controller in controllers) {
            val metadata = controller.metadata ?: continue
            val title = metadata.getString(android.media.MediaMetadata.METADATA_KEY_TITLE)
                ?: continue
            if (title.isBlank()) continue

            val playing = controller.playbackState?.state ==
                android.media.session.PlaybackState.STATE_PLAYING

            return NowPlaying(
                title = title,
                artist = metadata.getString(android.media.MediaMetadata.METADATA_KEY_ARTIST)
                    ?.takeIf { it.isNotBlank() },
                album = metadata.getString(android.media.MediaMetadata.METADATA_KEY_ALBUM)
                    ?.takeIf { it.isNotBlank() },
                player = appLabel(controller.packageName) ?: controller.packageName,
                playing = playing,
            )
        }
        return null
    }

    /** Human-readable app name for a package, when it resolves. */
    private fun appLabel(packageName: String): String? = try {
        val pm = context.packageManager
        pm.getApplicationLabel(pm.getApplicationInfo(packageName, 0)).toString()
    } catch (e: Exception) {
        null
    }

    /** Whether usage-access has been granted. */
    fun hasUsageAccess(): Boolean {
        val appOps = context.getSystemService(android.app.AppOpsManager::class.java)
            ?: return false
        val mode = if (android.os.Build.VERSION.SDK_INT >= 29) {
            appOps.unsafeCheckOpNoThrow(
                android.app.AppOpsManager.OPSTR_GET_USAGE_STATS,
                android.os.Process.myUid(),
                context.packageName,
            )
        } else {
            @Suppress("DEPRECATION")
            appOps.checkOpNoThrow(
                android.app.AppOpsManager.OPSTR_GET_USAGE_STATS,
                android.os.Process.myUid(),
                context.packageName,
            )
        }
        return mode == android.app.AppOpsManager.MODE_ALLOWED
    }

    /** Whether the notification listener is enabled for us. */
    fun hasNotificationAccess(): Boolean {
        val enabled = Settings.Secure.getString(
            context.contentResolver,
            "enabled_notification_listeners",
        ) ?: return false
        return enabled.split(':').any { it.contains(context.packageName) }
    }

    companion object {
        /** Settings screen for usage access. */
        fun usageAccessIntent(): Intent =
            Intent(Settings.ACTION_USAGE_ACCESS_SETTINGS)

        /** Settings screen for notification access. */
        fun notificationAccessIntent(): Intent =
            Intent("android.settings.ACTION_NOTIFICATION_LISTENER_SETTINGS")

        /**
         * Battery-optimisation exemption screen.
         *
         * Aggressive vendor battery managers are the main reason a foreground
         * service still gets killed, so the UI links here rather than pretending
         * the foreground service alone is enough.
         */
        fun batterySettingsIntent(context: Context): Intent {
            val manager = context.getSystemService(android.os.PowerManager::class.java)
            val exempt = manager?.isIgnoringBatteryOptimizations(context.packageName) == true
            return if (exempt) {
                Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS)
            } else {
                // Requesting directly is allowed only with the
                // REQUEST_IGNORE_BATTERY_OPTIMIZATIONS permission, which Play
                // restricts; sending the user to the list avoids that.
                Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS)
            }
        }

        fun isIgnoringBatteryOptimizations(context: Context): Boolean {
            val manager = context.getSystemService(android.os.PowerManager::class.java)
            return manager?.isIgnoringBatteryOptimizations(context.packageName) == true
        }

        /** Whether [SyncService] is currently running in this process. */
        @Suppress("DEPRECATION")
        fun isServiceRunning(context: Context): Boolean {
            val manager = context.getSystemService(ActivityManager::class.java)
                ?: return false
            return manager.getRunningServices(Int.MAX_VALUE)
                .any { it.service.className == SyncService::class.java.name }
        }
    }
}
