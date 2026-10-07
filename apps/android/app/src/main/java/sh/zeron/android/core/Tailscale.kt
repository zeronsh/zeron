package sh.zeron.android.core

import android.content.Context
import android.content.Intent
import android.net.Uri

/**
 * The Tailscale app on this phone: is it installed, does a network
 * interface hold a tailnet address (100.64.0.0/10 or fd7a:115c:a1e0::/48),
 * and open it (or its store page) from the failure hints. Package
 * visibility: AndroidManifest <queries> lists [PACKAGE].
 */
object Tailscale {
    const val PACKAGE = "com.tailscale.ipn"

    fun installed(context: Context): Boolean = runCatching {
        context.packageManager.getPackageInfo(PACKAGE, 0)
        true
    }.getOrDefault(false)

    /** Launch Tailscale; false when it isn't installed (or has no launcher activity). */
    fun open(context: Context): Boolean {
        val intent = context.packageManager.getLaunchIntentForPackage(PACKAGE) ?: return false
        intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        return runCatching { context.startActivity(intent) }.isSuccess
    }

    /** Tailscale's store page (Play Store app, else the web page). */
    fun openStore(context: Context): Boolean {
        val market = Intent(Intent.ACTION_VIEW, Uri.parse("market://details?id=$PACKAGE")).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        if (runCatching { context.startActivity(market) }.isSuccess) return true
        val web = Intent(Intent.ACTION_VIEW, Uri.parse("https://play.google.com/store/apps/details?id=$PACKAGE")).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        return runCatching { context.startActivity(web) }.isSuccess
    }

    /** Some interface of this phone holds a tailnet address (Tailscale's tun is up). */
    fun hasTailnetInterface(): Boolean = runCatching {
        java.net.NetworkInterface.getNetworkInterfaces()?.toList().orEmpty().any { nic ->
            nic.isUp && nic.inetAddresses.toList().any { EndpointKind.of(it.hostAddress.orEmpty().substringBefore('%')) == EndpointKind.TAILSCALE }
        }
    }.getOrDefault(false)
}
