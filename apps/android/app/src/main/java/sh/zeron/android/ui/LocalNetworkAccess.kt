package sh.zeron.android.ui

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.platform.LocalContext
import sh.zeron.android.core.AppModel

/**
 * Android 17 keeps this app off the local network unless it's allowed there, and a direct
 * connection to a computer on the same Wi-Fi fails silently without it (internet paths and the
 * relay don't need it). Asked once, the first time something here could use one, as iOS's system
 * prompt is. Declining changes nothing else: everything carries on over the relay.
 */
@Composable
fun RequestLocalNetworkOnce(model: AppModel, wanted: Boolean) {
    val context = LocalContext.current
    val request = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        // The paths that failed can work now: try them instead of waiting out their backoff.
        if (granted) runCatching { model.client.value?.networkPathChanged() }
    }
    LaunchedEffect(wanted) {
        if (wanted && shouldAskForLocalNetwork(context)) {
            prefs(context).edit().putBoolean(ASKED, true).apply()
            request.launch(Manifest.permission.ACCESS_LOCAL_NETWORK)
        }
    }
}

private fun shouldAskForLocalNetwork(context: Context): Boolean =
    Build.VERSION.SDK_INT >= 37 &&
        context.checkSelfPermission(Manifest.permission.ACCESS_LOCAL_NETWORK) != PackageManager.PERMISSION_GRANTED &&
        !prefs(context).getBoolean(ASKED, false)

private fun prefs(context: Context) = context.getSharedPreferences("settings", 0)

private const val ASKED = "localNetworkAsked"
