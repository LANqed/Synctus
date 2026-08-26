package dev.synctus.app

import android.service.notification.NotificationListenerService

/**
 * Exists purely to obtain notification-listener access.
 *
 * `MediaSessionManager.getActiveSessions` requires the caller to name an enabled
 * notification-listener component; this is that component. It deliberately does
 * nothing with the notifications themselves — no content is read, stored or
 * transmitted.
 */
class MediaListenerService : NotificationListenerService()
