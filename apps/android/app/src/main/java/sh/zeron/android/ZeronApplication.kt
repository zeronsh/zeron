package sh.zeron.android

import android.app.Application
import sh.zeron.android.core.AppModel
import sh.zeron.android.core.Fonts
import sh.zeron.android.design.LocalAssets

class ZeronApplication : Application() {
    lateinit var model: AppModel
        private set

    override fun onCreate() {
        super.onCreate()
        LocalAssets.manager = assets
        Fonts.init(assets)
        model = AppModel(this)
    }
}
