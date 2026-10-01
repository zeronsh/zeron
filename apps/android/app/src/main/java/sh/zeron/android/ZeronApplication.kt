package sh.zeron.android

import android.app.Application

class ZeronApplication : Application() {
    override fun onCreate() {
        super.onCreate()
        // JNA loads libzeron_mobile.so from the extracted native-lib dir.
        System.setProperty("jna.nosys", "true")
    }
}
