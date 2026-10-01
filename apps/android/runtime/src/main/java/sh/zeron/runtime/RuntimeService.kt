package sh.zeron.runtime

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch

/**
 * Foreground (specialUse) service that keeps the guest alive: while it
 * exists the runtime bootstraps and supervises the engine; destroying it
 * stops the engine.
 */
class RuntimeService : Service() {
    private lateinit var runtime: GuestRuntime
    private val scope = MainScope()
    private var wakeLock: PowerManager.WakeLock? = null

    override fun onCreate() {
        super.onCreate()
        runtime = ZeronRuntime.impl(this)
        createChannel()
        goForeground(runtime.state.value)
        // Agents work with the screen off; the user opted in by starting
        // the engine, and the lock goes with the service.
        wakeLock = getSystemService(PowerManager::class.java)
            ?.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "zeron:runtime")
            ?.apply { setReferenceCounted(false); acquire() }
        runtime.onServiceStarted()
        scope.launch {
            runtime.state.collect { state ->
                getSystemService(NotificationManager::class.java)?.notify(NOTIFICATION_ID, notification(state))
            }
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP) {
            stopSelf()
            return START_NOT_STICKY
        }
        // Every startForegroundService() must be answered with startForeground().
        goForeground(runtime.state.value)
        return START_STICKY
    }

    override fun onDestroy() {
        scope.cancel()
        runtime.onServiceStopped()
        wakeLock?.release()
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    private fun goForeground(state: RuntimeState) {
        val type = if (Build.VERSION.SDK_INT >= 34) ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE else 0
        ServiceCompat.startForeground(this, NOTIFICATION_ID, notification(state), type)
    }

    private fun createChannel() {
        val channel = NotificationChannel(CHANNEL_ID, "On-device engine", NotificationManager.IMPORTANCE_LOW)
            .apply { description = "Shown while Zeron's agents run on this device" }
        getSystemService(NotificationManager::class.java)?.createNotificationChannel(channel)
    }

    private fun notification(state: RuntimeState): Notification {
        val text = when (state) {
            is RuntimeState.Bootstrapping ->
                state.step + (state.progress?.let { " · ${(it * 100).toInt()}%" } ?: "")
            RuntimeState.Starting -> "Starting the engine…"
            is RuntimeState.Running -> "Running on this device"
            is RuntimeState.Failed -> state.reason
            RuntimeState.NotInstalled, RuntimeState.Stopped -> "Stopping…"
        }
        val flags = PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        val stop = PendingIntent.getService(
            this, 0, Intent(this, RuntimeService::class.java).setAction(ACTION_STOP), flags,
        )
        val open = packageManager.getLaunchIntentForPackage(packageName)
            ?.let { PendingIntent.getActivity(this, 0, it, flags) }
        return NotificationCompat.Builder(this, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_zeron_runtime)
            .setContentTitle("Zeron engine")
            .setContentText(text)
            .setStyle(NotificationCompat.BigTextStyle().bigText(text))
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setSilent(true)
            .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
            .apply {
                if (state is RuntimeState.Bootstrapping) {
                    setProgress(100, ((state.progress ?: 0f) * 100).toInt(), state.progress == null)
                }
            }
            .setContentIntent(open)
            .addAction(0, "Stop", stop)
            .build()
    }

    companion object {
        const val CHANNEL_ID = "zeron_runtime"
        private const val NOTIFICATION_ID = 0x2e70
        private const val ACTION_STOP = "sh.zeron.runtime.STOP"
    }
}
