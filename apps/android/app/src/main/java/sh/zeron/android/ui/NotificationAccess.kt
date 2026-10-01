package sh.zeron.android.ui

import android.Manifest
import android.content.Intent
import android.os.Build
import android.provider.Settings
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalContext
import sh.zeron.android.core.AppModel

/**
 * Session alerts in the background need the notification permission (API 33+).
 * It is asked for once, in context (the first message you send), and can be
 * asked again from Settings > Sounds & haptics.
 */
class NotificationAccess(val granted: Boolean, val askOnce: () -> Unit, val ask: () -> Unit, val settings: () -> Unit)

@Composable
fun rememberNotificationAccess(model: AppModel): NotificationAccess {
    val context = LocalContext.current
    var granted by remember { mutableStateOf(model.notifier.permitted) }
    androidx.lifecycle.compose.LifecycleResumeEffect(Unit) {
        granted = model.notifier.permitted
        onPauseOrDispose {}
    }
    var explicit by remember { mutableStateOf(false) }
    fun openSettings() {
        runCatching {
            context.startActivity(
                Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS)
                    .putExtra(Settings.EXTRA_APP_PACKAGE, context.packageName)
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            )
        }
    }
    val launcher = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { ok ->
        granted = ok
        // Denied for good (the system no longer shows the prompt): send people to the switch instead.
        if (!ok && explicit) openSettings()
    }
    return remember(model, launcher, granted) {
        NotificationAccess(
            granted = granted,
            askOnce = {
                if (Build.VERSION.SDK_INT >= 33 && !model.notifier.permitted && !model.notificationsAsked) {
                    model.notificationsAsked = true
                    explicit = false
                    launcher.launch(Manifest.permission.POST_NOTIFICATIONS)
                }
            },
            ask = {
                model.notificationsAsked = true
                explicit = true
                launcher.launch(Manifest.permission.POST_NOTIFICATIONS)
            },
            settings = ::openSettings,
        )
    }
}
