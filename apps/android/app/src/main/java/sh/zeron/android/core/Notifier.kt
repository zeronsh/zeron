package sh.zeron.android.core

import android.Manifest
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.media.AudioAttributes
import android.net.Uri
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import sh.zeron.android.MainActivity
import sh.zeron.android.R
import sh.zeron.android.feedback.CueCategory
import sh.zeron.android.feedback.FeedbackSettings
import sh.zeron.android.feedback.SessionAlerts
import sh.zeron.android.feedback.SessionEvent

/**
 * Local notifications, posted only when Android permits them:
 *
 *  - Save to Downloads progress from the developer tools.
 *  - Session events while the app is in the background (finished, needs your
 *    input, failed), sounding the same cues the app plays in front: the
 *    desktop's done / request / attention chimes (the mastered `fx_chime_*` copies) as channel sounds, with
 *    vibration patterns matching the in-app haptics.
 *
 * Channel sounds are immutable once a channel exists, so session channels
 * carry a version in their id ([CHANNEL_VERSION]); [migrateChannels] deletes
 * the ones from other versions. One channel exists per kind and per
 * sound / vibration combination the in-app switches ask for, created lazily.
 */
class Notifier(private val context: Context, private val settings: () -> FeedbackSettings = { FeedbackSettings() }) : SessionAlerts {
    private val manager = context.getSystemService(NotificationManager::class.java)

    init {
        manager.createNotificationChannel(
            NotificationChannel(DOWNLOADS_CHANNEL, "Downloads", NotificationManager.IMPORTANCE_LOW).apply {
                description = "Files saved to Downloads from your devices"
            },
        )
        migrateChannels()
    }

    /** Session notification kinds: wording, channel importance, chime and vibration. */
    enum class Kind(
        val key: String,
        val label: String,
        val description: String,
        val importance: Int,
        val sound: String,
        val category: CueCategory,
        /** Off / on pairs in ms, matching the in-app haptic of the same moment. */
        val pattern: LongArray,
        val event: SessionEvent,
    ) {
        Done("done", "Task completed", "A session finished its turn", NotificationManager.IMPORTANCE_DEFAULT, "fx_chime_done", CueCategory.Completion, longArrayOf(0, 24, 40, 30), SessionEvent.Done),
        Input("input", "Input required", "A session is waiting on your answer or approval", NotificationManager.IMPORTANCE_HIGH, "fx_chime_request", CueCategory.Input, longArrayOf(0, 18, 90, 18), SessionEvent.NeedsInput),
        Failed("failed", "Errors", "A session failed", NotificationManager.IMPORTANCE_HIGH, "fx_chime_attention", CueCategory.Errors, longArrayOf(0, 35, 55, 45), SessionEvent.Failed),
    }

    /** A session channel id, e.g. `session-done-v1-sv` (sound + vibration), `-s`, `-v` or `-q` (silent). */
    fun channelId(kind: Kind, sound: Boolean, vibrate: Boolean): String =
        "$SESSION_PREFIX${kind.key}-v$CHANNEL_VERSION-" + (if (sound) "s" else "") + (if (vibrate) "v" else "") + (if (!sound && !vibrate) "q" else "")

    private fun ensureChannel(kind: Kind, sound: Boolean, vibrate: Boolean): String {
        val id = channelId(kind, sound, vibrate)
        if (manager.getNotificationChannel(id) != null) return id
        val suffix = when {
            sound && vibrate -> ""
            sound -> " (sound only)"
            vibrate -> " (vibration only)"
            else -> " (silent)"
        }
        val channel = NotificationChannel(id, kind.label + suffix, if (sound || vibrate) kind.importance else NotificationManager.IMPORTANCE_LOW)
        channel.description = kind.description
        if (sound) {
            channel.setSound(
                Uri.parse("android.resource://${context.packageName}/raw/${kind.sound}"),
                AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_NOTIFICATION).setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION).build(),
            )
        } else {
            channel.setSound(null, null)
        }
        channel.enableVibration(vibrate)
        if (vibrate) channel.vibrationPattern = kind.pattern
        manager.createNotificationChannel(channel)
        return id
    }

    /**
     * The channel of a notification that has no kind of its own (a finished save): the completion chime is the app's
     * default sound, with the Completion switch and the vibration setting choosing the variant, like the session ones.
     */
    private fun completionChannel(sound: Boolean, vibrate: Boolean): String = ensureChannel(Kind.Done, sound, vibrate)

    /** Remove session channels left by other versions (their sounds cannot be edited in place). */
    fun migrateChannels() {
        val current = "-v$CHANNEL_VERSION-"
        manager.notificationChannels
            .filter { it.id.startsWith(SESSION_PREFIX) && !it.id.contains(current) }
            .forEach { manager.deleteNotificationChannel(it.id) }
    }

    /** A session event while the app is in the background; the in-app switches pick the channel. */
    override fun alert(chatId: String, event: SessionEvent) {
        if (!permitted) {
            android.util.Log.d("ZeronFeedback", "notification $event for $chatId skipped: notifications not allowed")
            return
        }
        val kind = Kind.entries.first { it.event == event }
        val s = settings()
        val sound = s.allows(kind.category)
        val vibrate = s.hapticsOn
        val row = runCatching { (context.applicationContext as sh.zeron.android.ZeronApplication).model.row(chatId) }.getOrNull()
        val title = row?.title ?: "Zeron"
        val text = when (event) {
            SessionEvent.Done -> "Finished"
            SessionEvent.NeedsInput -> "Needs your input"
            SessionEvent.Failed -> "Something went wrong"
        }
        val open = Intent(context, MainActivity::class.java)
            .putExtra("route", "chat:$chatId")
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
        val tap = PendingIntent.getActivity(context, "session:$chatId".hashCode(), open, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        val n = NotificationCompat.Builder(context, ensureChannel(kind, sound, vibrate))
            .setSmallIcon(R.drawable.ic_stat_zeron)
            .setContentTitle(title)
            .setContentText(text)
            .setCategory(if (event == SessionEvent.NeedsInput) NotificationCompat.CATEGORY_MESSAGE else NotificationCompat.CATEGORY_STATUS)
            .setContentIntent(tap)
            .setAutoCancel(true)
            .build()
        android.util.Log.d("ZeronFeedback", "notification $event for $chatId on channel ${n.channelId}")
        try {
            NotificationManagerCompat.from(context).notify("session:$chatId".hashCode(), n)
        } catch (_: SecurityException) {
        }
    }

    /** The app came to the front: its sessions are on screen, clear the alerts. */
    fun clearSessionAlerts() = manager.activeNotifications.filter { it.notification.channelId?.startsWith(SESSION_PREFIX) == true }.forEach { manager.cancel(it.id) }

    val permitted: Boolean
        get() = Build.VERSION.SDK_INT < 33 ||
            ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED

    /** Save to Downloads from the developer tools: progress, then where it landed. */
    fun download(id: String, title: String, text: String, fraction: Float?, done: Boolean, open: Intent? = null, ok: Boolean = true) {
        val tap = open?.let {
            PendingIntent.getActivity(context, "download:$id".hashCode(), it.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK), PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        }
        // Progress stays quiet. A finished save has no kind of its own, so it plays the app's default sound: the
        // completion chime (on the Done channel, under the same switches). A failed one stays silent.
        val s = settings()
        val channel = if (done && ok) completionChannel(s.allows(Kind.Done.category), s.hapticsOn) else DOWNLOADS_CHANNEL
        val n = NotificationCompat.Builder(context, channel)
            .setSmallIcon(R.drawable.ic_stat_zeron)
            .setContentTitle(title)
            .setContentText(text)
            .setCategory(if (done) NotificationCompat.CATEGORY_STATUS else NotificationCompat.CATEGORY_PROGRESS)
            .apply {
                if (!done) setProgress(100, ((fraction ?: 0f) * 100).toInt(), fraction == null).setOngoing(true).setOnlyAlertOnce(true).setSilent(true)
                else setAutoCancel(true)
                if (tap != null) setContentIntent(tap)
            }
            .build()
        if (!permitted) return
        try {
            NotificationManagerCompat.from(context).notify("download:$id".hashCode(), n)
        } catch (_: SecurityException) {
        }
    }

    companion object {
        const val DOWNLOADS_CHANNEL = "downloads"
        const val SESSION_PREFIX = "session-"

        /** Bump when a session channel's sound or pattern changes: channel sounds are immutable once created. */
        const val CHANNEL_VERSION = 2
    }
}
