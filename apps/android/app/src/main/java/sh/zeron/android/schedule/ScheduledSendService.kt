package sh.zeron.android.schedule

import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * Foreground (dataSync) service that sends due scheduled messages, posts
 * the result, and stops when nothing is left in flight.
 */
class ScheduledSendService : Service() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private val inFlight = mutableSetOf<String>()

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val type = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC else 0
        ServiceCompat.startForeground(this, ScheduledNotifications.PROGRESS_ID, ScheduledNotifications.progress(this), type)
        val id = intent?.getStringExtra(ScheduledAlarms.EXTRA_ID)
        if (id == null || !inFlight.add(id)) {
            if (inFlight.isEmpty()) stopSelf()
            return START_NOT_STICKY
        }
        scope.launch {
            val result = ScheduledSender(applicationContext).deliver(id)
            ScheduledNotifications.result(applicationContext, result)
            withContext(Dispatchers.Main) {
                inFlight.remove(id)
                if (inFlight.isEmpty()) {
                    ServiceCompat.stopForeground(this@ScheduledSendService, ServiceCompat.STOP_FOREGROUND_REMOVE)
                    stopSelf()
                }
            }
        }
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        scope.cancel()
        super.onDestroy()
    }

    companion object {
        fun start(context: Context, id: String) {
            ContextCompat.startForegroundService(
                context,
                Intent(context, ScheduledSendService::class.java).putExtra(ScheduledAlarms.EXTRA_ID, id),
            )
        }
    }
}
