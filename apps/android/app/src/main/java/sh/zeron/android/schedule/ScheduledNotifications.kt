package sh.zeron.android.schedule

import sh.zeron.android.core.AppLanguage
import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import sh.zeron.android.MainActivity
import sh.zeron.android.R

/** Notifications for scheduled sends: the ongoing "Sending…" and the result. */
object ScheduledNotifications {
    private const val CHANNEL_PROGRESS = "scheduled-progress"
    private const val CHANNEL_RESULT = "scheduled-result"
    const val PROGRESS_ID = 4100

    private fun channels(context: Context) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        val nm = context.getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(
            NotificationChannel(CHANNEL_PROGRESS, AppLanguage.string(context, R.string.schedule_channel_progress), NotificationManager.IMPORTANCE_LOW),
        )
        nm.createNotificationChannel(
            NotificationChannel(CHANNEL_RESULT, AppLanguage.string(context, R.string.schedule_channel_result), NotificationManager.IMPORTANCE_DEFAULT),
        )
    }

    private fun openApp(context: Context): PendingIntent = PendingIntent.getActivity(
        context,
        0,
        Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP),
        PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
    )

    fun progress(context: Context): Notification {
        channels(context)
        return NotificationCompat.Builder(context, CHANNEL_PROGRESS)
            .setSmallIcon(R.drawable.ic_stat_schedule)
            .setContentTitle(AppLanguage.string(context, R.string.schedule_notif_sending))
            .setProgress(0, 0, true)
            .setOngoing(true)
            .setSilent(true)
            .setContentIntent(openApp(context))
            .build()
    }

    fun result(context: Context, result: ScheduledSender.Result) {
        val (message, title, body) = when (result) {
            is ScheduledSender.Result.Sent -> Triple(result.message, AppLanguage.string(context, R.string.schedule_notif_sent), excerpt(result.message))
            is ScheduledSender.Result.Pending -> Triple(result.message, AppLanguage.string(context, R.string.schedule_notif_pending), AppLanguage.string(context, R.string.schedule_notif_pending_body))
            is ScheduledSender.Result.Failed -> Triple(
                result.message,
                AppLanguage.string(context, R.string.schedule_notif_failed),
                AppLanguage.string(context, R.string.schedule_notif_failed_body, result.reason, result.message.text),
            )
            ScheduledSender.Result.Gone -> return
        }
        if (Build.VERSION.SDK_INT >= 33 &&
            ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) return
        channels(context)
        val n = NotificationCompat.Builder(context, CHANNEL_RESULT)
            .setSmallIcon(R.drawable.ic_stat_schedule)
            .setContentTitle(title)
            .setContentText(body)
            .setStyle(NotificationCompat.BigTextStyle().bigText(body))
            .setSubText(message.chatTitle.ifBlank { null })
            .setAutoCancel(true)
            .setContentIntent(openApp(context))
            .build()
        NotificationManagerCompat.from(context).notify(message.id.hashCode(), n)
    }

    private fun excerpt(message: ScheduledMessage): String =
        message.text.lineSequence().firstOrNull().orEmpty().let { if (it.length > 120) it.take(119) + "…" else it }
}
