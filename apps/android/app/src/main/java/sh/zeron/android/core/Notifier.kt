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
 *  - Save to Downloads progress from the developer tools, and the progress of
 *    an incoming file transfer (silent, ongoing).
 *  - Session events while the app is in the background (finished, needs your
 *    input, failed), sounding the same cues the app plays in front: the
 *    desktop's done / request / attention chimes (the mastered `fx_chime_*` copies) as channel sounds, with
 *    vibration patterns matching the in-app haptics.
 *  - File-transfer events (another device asks to send, a file arrived, a
 *    transfer failed), which reuse the request / done / attention cues. They
 *    are posted in front as well (they carry Accept / Decline and open the
 *    file), but silently there: the app plays its own cue.
 *
 * Channel sounds are immutable once a channel exists, so alert channels
 * carry a version in their id ([CHANNEL_VERSION]); [migrateChannels] deletes
 * the ones from other versions. One channel exists per kind and per
 * sound / vibration combination the in-app switches ask for, created lazily.
 */
class Notifier(
    private val context: Context,
    private val settings: () -> FeedbackSettings = { FeedbackSettings() },
    private val foreground: () -> Boolean = { false },
) : SessionAlerts {
    private val manager = context.getSystemService(NotificationManager::class.java)

    init {
        manager.createNotificationChannel(
            NotificationChannel(DOWNLOADS_CHANNEL, "Downloads and transfers", NotificationManager.IMPORTANCE_LOW).apply {
                description = "Progress of files saved to Downloads and of incoming transfers"
            },
        )
        migrateChannels()
    }

    /** Notification kinds: wording, channel importance, chime and vibration. */
    enum class Kind(
        val key: String,
        val label: String,
        val description: String,
        val importance: Int,
        val sound: String,
        val category: CueCategory,
        /** Off / on pairs in ms, matching the in-app haptic of the same moment. */
        val pattern: LongArray,
        val event: SessionEvent?,
        val prefix: String = SESSION_PREFIX,
    ) {
        Done("done", "Task completed", "A session finished its turn", NotificationManager.IMPORTANCE_DEFAULT, "fx_chime_done", CueCategory.Completion, longArrayOf(0, 24, 40, 30), SessionEvent.Done),
        Input("input", "Input required", "A session is waiting on your answer or approval", NotificationManager.IMPORTANCE_HIGH, "fx_chime_request", CueCategory.Input, longArrayOf(0, 18, 90, 18), SessionEvent.NeedsInput),
        Failed("failed", "Errors", "A session failed", NotificationManager.IMPORTANCE_HIGH, "fx_chime_attention", CueCategory.Errors, longArrayOf(0, 35, 55, 45), SessionEvent.Failed),

        /** Another device asks to send this phone files (request chime). */
        TransferAsk("ask", "Transfer requests", "Another device wants to send you files", NotificationManager.IMPORTANCE_HIGH, "fx_chime_request", CueCategory.Input, longArrayOf(0, 18, 90, 18), null, TRANSFER_PREFIX),

        /** Files arrived (done chime). */
        TransferReceived("received", "Files received", "Files from your other devices arrived", NotificationManager.IMPORTANCE_DEFAULT, "fx_chime_done", CueCategory.Completion, longArrayOf(0, 24, 40, 30), null, TRANSFER_PREFIX),

        /** A transfer failed, was cancelled or declined by the other side (attention chime). */
        TransferFailed("failed", "Transfer problems", "A file transfer failed or was declined", NotificationManager.IMPORTANCE_HIGH, "fx_chime_attention", CueCategory.Errors, longArrayOf(0, 35, 55, 45), null, TRANSFER_PREFIX),
    }

    /** A channel id, e.g. `session-done-v1-sv` (sound + vibration), `-s`, `-v` or `-q` (silent). */
    fun channelId(kind: Kind, sound: Boolean, vibrate: Boolean): String =
        "${kind.prefix}${kind.key}-v$CHANNEL_VERSION-" + (if (sound) "s" else "") + (if (vibrate) "v" else "") + (if (!sound && !vibrate) "q" else "")

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

    /**
     * Remove alert channels left by other versions (their sounds cannot be
     * edited in place), and the unversioned `sessions` / `transfers` channels
     * of earlier builds.
     */
    fun migrateChannels() {
        val current = "-v$CHANNEL_VERSION-"
        manager.notificationChannels
            .filter { (it.id.startsWith(SESSION_PREFIX) || it.id.startsWith(TRANSFER_PREFIX)) && !it.id.contains(current) || it.id in LEGACY_CHANNELS }
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

    // ── file transfers ─────────────────────────────────────────────────────
    // Progress is silent. The ask / received / ended notifications are posted
    // in front and behind (they carry actions and open the file); only behind
    // do they sound (the channel's chime): in front the app plays the same
    // cue itself (feedback/TransferFeedback), so the notification stays quiet.

    private fun transferId(id: String) = "transfer:$id".hashCode()

    private fun openTransfers(id: String): PendingIntent {
        val open = Intent(context, MainActivity::class.java)
            .putExtra("route", "transfers")
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
        return PendingIntent.getActivity(context, transferId(id), open, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
    }

    private fun transferBuilder(t: Transfers.Transfer, kind: Kind?): NotificationCompat.Builder {
        val channel = if (kind == null) DOWNLOADS_CHANNEL else {
            val s = settings()
            ensureChannel(kind, s.allows(kind.category), s.haptics)
        }
        return NotificationCompat.Builder(context, channel)
            .setSmallIcon(R.drawable.ic_stat_zeron)
            .setContentIntent(openTransfers(t.id))
            .setCategory(NotificationCompat.CATEGORY_PROGRESS)
            .apply { if (kind != null && foreground()) setSilent(true) }
    }

    private fun notify(id: String, n: android.app.Notification) {
        if (!permitted) return
        try {
            NotificationManagerCompat.from(context).notify(transferId(id), n)
        } catch (_: SecurityException) {
        }
    }

    /** An incoming transfer in flight: one ongoing notification with its progress. */
    fun transferProgress(t: Transfers.Transfer) {
        val n = transferBuilder(t, null)
            .setContentTitle("Receiving ${t.title}")
            .setContentText(listOf("From ${t.peerDeviceName}", Transfers.detail(t)).joinToString(" · "))
            .setProgress(100, (t.fraction * 100).toInt(), t.state != Transfers.State.Transferring || t.totalBytes == 0L)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setSilent(true)
            .build()
        notify(t.id, n)
    }

    /** Another device asks to send (this phone asks before accepting). */
    fun transferAsk(t: Transfers.Transfer) {
        fun action(accept: Boolean): PendingIntent {
            val intent = Intent(context, TransferActionReceiver::class.java)
                .setAction(if (accept) TransferActionReceiver.ACCEPT else TransferActionReceiver.DECLINE)
                .putExtra(TransferActionReceiver.EXTRA_ID, t.id)
            return PendingIntent.getBroadcast(context, transferId(t.id) + if (accept) 1 else 2, intent, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        }
        val n = transferBuilder(t, Kind.TransferAsk)
            .setContentTitle("${t.peerDeviceName} wants to send you ${t.title}")
            .setContentText("${Transfers.files(t.fileCount)} · ${Transfers.bytes(t.totalBytes)}")
            .setCategory(NotificationCompat.CATEGORY_MESSAGE)
            .addAction(0, "Accept", action(true))
            .addAction(0, "Decline", action(false))
            .setAutoCancel(true)
            .build()
        notify(t.id, n)
    }

    /** Received: tapping opens the file (or Transfers when there's no single file to open). */
    fun transferReceived(t: Transfers.Transfer, open: Intent?, problem: String? = null) {
        val tap = open?.let {
            PendingIntent.getActivity(context, transferId(t.id), it.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK), PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        }
        val n = transferBuilder(t, if (problem == null) Kind.TransferReceived else Kind.TransferFailed)
            .setContentTitle("Received ${t.title}")
            .setContentText(problem ?: listOf("From ${t.peerDeviceName}", Transfers.files(t.fileCount), Transfers.bytes(t.totalBytes), "In Downloads").joinToString(" · "))
            .apply { if (tap != null) setContentIntent(tap) }
            .setCategory(NotificationCompat.CATEGORY_STATUS)
            .setAutoCancel(true)
            .build()
        notify(t.id, n)
    }

    /** Failed, cancelled or declined after we showed it. */
    fun transferEnded(t: Transfers.Transfer) {
        val n = transferBuilder(t, Kind.TransferFailed)
            .setContentTitle("${Transfers.stateLabel(t)}: ${t.title}")
            .setContentText(t.error ?: "From ${t.peerDeviceName}")
            .setCategory(NotificationCompat.CATEGORY_STATUS)
            .setAutoCancel(true)
            .build()
        notify(t.id, n)
    }

    fun cancelTransfer(id: String) {
        NotificationManagerCompat.from(context).cancel(transferId(id))
    }

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
        const val TRANSFER_PREFIX = "transfer-"

        /** Channels of builds before the versioned ones. */
        private val LEGACY_CHANNELS = setOf("sessions", "transfers")

        /** Bump when a session channel's sound or pattern changes: channel sounds are immutable once created. */
        const val CHANNEL_VERSION = 2
    }
}
