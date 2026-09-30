package sh.zeron.android.core

import android.Manifest
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
import sh.zeron.android.R

/**
 * Local notifications: Save to Downloads progress from the developer tools
 * (the tools show the same progress on screen). Posted only when Android
 * permits notifications for the app.
 */
class Notifier(private val context: Context) {
    init {
        val manager = context.getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(
            NotificationChannel(DOWNLOADS_CHANNEL, "Downloads", NotificationManager.IMPORTANCE_LOW).apply {
                description = "Files saved to Downloads from your devices"
            },
        )
    }

    val permitted: Boolean
        get() = Build.VERSION.SDK_INT < 33 ||
            ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED

    /** Save to Downloads from the developer tools: progress, then where it landed. */
    fun download(id: String, title: String, text: String, fraction: Float?, done: Boolean, open: Intent? = null) {
        val tap = open?.let {
            PendingIntent.getActivity(context, "download:$id".hashCode(), it.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK), PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        }
        val n = NotificationCompat.Builder(context, DOWNLOADS_CHANNEL)
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
    }
}
