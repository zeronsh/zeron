package sh.zeron.runtime

import android.Manifest
import android.annotation.SuppressLint
import android.app.Activity
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.PowerManager
import android.provider.Settings
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat

/** Permission and system-setting helpers the app calls around start(). */
object RuntimePermissions {
    const val NOTIFICATIONS_REQUEST_CODE = 0x2e71

    /** Android 13+: without it the foreground notification is hidden (the engine still runs). */
    fun needsNotificationPermission(context: Context): Boolean =
        Build.VERSION.SDK_INT >= 33 &&
            ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) !=
            PackageManager.PERMISSION_GRANTED

    /** Result arrives in the activity's onRequestPermissionsResult. */
    fun requestNotificationPermission(activity: Activity, requestCode: Int = NOTIFICATIONS_REQUEST_CODE) {
        if (needsNotificationPermission(activity) && Build.VERSION.SDK_INT >= 33) {
            ActivityCompat.requestPermissions(activity, arrayOf(Manifest.permission.POST_NOTIFICATIONS), requestCode)
        }
    }

    fun isIgnoringBatteryOptimizations(context: Context): Boolean =
        context.getSystemService(PowerManager::class.java)?.isIgnoringBatteryOptimizations(context.packageName) == true

    /**
     * Shows the system "stop optimising battery usage?" dialog. Call when the
     * user starts the engine (docs/android.md § Android platform constraints).
     * Returns false if already exempt or the dialog isn't available.
     */
    @SuppressLint("BatteryLife")
    fun requestIgnoreBatteryOptimizations(activity: Activity): Boolean {
        if (isIgnoringBatteryOptimizations(activity)) return false
        val intent = Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS)
            .setData(Uri.parse("package:${activity.packageName}"))
        return try {
            activity.startActivity(intent)
            true
        } catch (_: Exception) {
            false
        }
    }

    /** Where "Disable child process restrictions" lives, for a Failed phantom-kill state. */
    fun developerOptionsIntent(): Intent =
        Intent(Settings.ACTION_APPLICATION_DEVELOPMENT_SETTINGS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
}
