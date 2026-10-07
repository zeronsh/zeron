package sh.zeron.android.schedule

import android.app.AlarmManager
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch

/** Alarm fired → foreground service; boot/update/permission change → re-arm alarms. */
class ScheduledSendReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        when (intent.action) {
            ScheduledAlarms.ACTION_FIRE -> {
                val id = intent.getStringExtra(ScheduledAlarms.EXTRA_ID) ?: return
                try {
                    ScheduledSendService.start(context, id)
                } catch (t: Exception) {
                    // Android 12+ refuses a background foreground-service start
                    // when the alarm wasn't exact. Send from here instead, inside
                    // the broadcast's time allowance.
                    val pending = goAsync()
                    CoroutineScope(SupervisorJob() + Dispatchers.Default).launch {
                        try {
                            val app = context.applicationContext
                            ScheduledNotifications.result(app, ScheduledSender(app).deliver(id, budgetMs = FALLBACK_BUDGET_MS))
                        } finally {
                            pending.finish()
                        }
                    }
                }
            }
            Intent.ACTION_BOOT_COMPLETED,
            Intent.ACTION_MY_PACKAGE_REPLACED,
            AlarmManager.ACTION_SCHEDULE_EXACT_ALARM_PERMISSION_STATE_CHANGED,
            -> ScheduledAlarms.restoreAll(context)
        }
    }

    private companion object {
        /** Background broadcasts get about a minute. */
        const val FALLBACK_BUDGET_MS = 30_000L
    }
}
