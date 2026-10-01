package sh.zeron.android.core

import sh.zeron.android.design.ZIcons

/** Pure device logic shared by the pickers and Settings (JVM-tested). */
object DeviceIdentity {
    /** A device's glyph: phones and tablets by platform, servers, else a laptop. */
    fun icon(platform: String): Int = when (platform) {
        "android", "ios", "ipados" -> ZIcons.Phone
        "linux" -> ZIcons.Server
        else -> ZIcons.Laptop
    }
}
