package sh.zeron.android.screenshots

import android.app.Application
import android.os.Looper
import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.compose.ui.test.onRoot
import androidx.lifecycle.ViewModelProvider
import androidx.test.core.app.ActivityScenario
import androidx.test.core.app.ApplicationProvider
import com.github.takahirom.roborazzi.captureRoboImage
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import sh.zeron.android.BuildConfig
import sh.zeron.android.MainActivity
import sh.zeron.android.R
import sh.zeron.android.core.Updater
import sh.zeron.android.core.ZeronModel

/**
 * The update download controls: the sheet the badge opens while
 * downloading, the 换个镜像 (Switch mirror) list, and the update screen's two buttons.
 * Output: update-download/<lang>-*.png under the renders dir.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [34], qualifiers = "w411dp-h891dp-night-xxhdpi")
open class DownloadControlsScreenshotTest {
    protected open val lang: String = "en"

    @get:Rule
    val compose = createEmptyComposeRule()

    @Before
    fun gate() {
        Screenshots.assumeEnabled()
        Screenshots.assumeHostCore()
    }

    @Test
    fun downloadControls() {
        val app = ApplicationProvider.getApplicationContext<Application>()
        org.robolectric.shadows.ShadowBuild.setModel("Pixel 8")
        FakeAndroidKeyStore.install()
        app.getSharedPreferences("zeron-update", 0).edit().putLong("lastCheck", System.currentTimeMillis()).putBoolean("autoUpdate", false).commit()
        val scenario = ActivityScenario.launch(MainActivity::class.java)
        lateinit var model: ZeronModel
        scenario.onActivity { model = ViewModelProvider(it)[ZeronModel::class.java] }
        settleUntil("demo workspace") { model.phase == ZeronModel.Phase.Ready && model.workspace != null }
        model.applyListMode(ZeronModel.ListMode.Project)
        model.updater.mirror = "https://mirror.example.cn/"
        val release = Updater.Release(
            "round5-9", "round5-9", "Fixture notes", BuildConfig.VERSION_CODE.toLong() + 1,
            "zeron-android-round5-9.apk", "", "https://github.com/villatothesea/zeron-android-app/releases/download/round5-9/zeron-android-round5-9.apk",
            41_900_000, "", null,
        )
        model.updateRelease = release
        model.updateProgress = 0.37f
        model.updateStatus = ZeronModel.UpdateStatus("ghfast.top", 15_503_000, 41_900_000, 1_240_000, "https://ghfast.top/")
        model.sourceStats = mapOf(
            "github" to ZeronModel.SourceStat(error = app.getString(R.string.update_reason_timeout)),
            "https://ghfast.top/" to ZeronModel.SourceStat(bytesPerSec = 1_240_000),
            "https://gh-proxy.com/" to ZeronModel.SourceStat(bytesPerSec = 386_000),
        )
        for ((mode, suffix) in listOf(2 to "dark", 1 to "light")) {
            model.applyAppearance(mode)
            model.showDownloadSheet = true
            settle()
            capture("$lang-1-sheet-$suffix.png")
            model.showDownloadSheet = false
            model.showSourcePicker = true
            settle()
            capture("$lang-2-switch-mirror-$suffix.png")
            model.showSourcePicker = false
            model.showUpdate = true
            settle()
            capture("$lang-3-update-screen-$suffix.png")
            model.showUpdate = false
            settle()
        }
        model.updater.mirror = null
        model.updateRelease = null
        model.updateProgress = null
        model.updateStatus = null
        model.applyAppearance(2)
        scenario.close()
    }

    private fun capture(name: String) {
        compose.onRoot().captureRoboImage(Screenshots.path("update-download/$name"))
    }

    private fun settle(ms: Long = 800) {
        val end = System.currentTimeMillis() + ms
        while (System.currentTimeMillis() < end) {
            shadowOf(Looper.getMainLooper()).idleFor(java.time.Duration.ofMillis(50))
            compose.mainClock.advanceTimeBy(50)
            Thread.sleep(20)
        }
        compose.waitForIdle()
    }

    private fun settleUntil(what: String, timeoutMs: Long = 60_000, done: () -> Boolean) {
        val end = System.currentTimeMillis() + timeoutMs
        while (!done()) {
            check(System.currentTimeMillis() < end) { "timed out waiting for $what" }
            settle(200)
        }
    }
}

/** [DownloadControlsScreenshotTest] in Simplified Chinese. */
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [34], qualifiers = "zh-rCN-w411dp-h891dp-night-xxhdpi")
class DownloadControlsZhScreenshotTest : DownloadControlsScreenshotTest() {
    override val lang = "zh"
}
