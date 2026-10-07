package sh.zeron.android

import android.app.Application

class ZeronApplication : Application() {
    override fun onCreate() {
        super.onCreate()
        // Local-only crash log (Settings > About > Crash logs); first, so a
        // crash anywhere after this is recorded.
        sh.zeron.android.core.CrashLog.install(this)
        // JNA loads libzeron_mobile.so from the extracted native-lib dir.
        System.setProperty("jna.nosys", "true")
    }
}
