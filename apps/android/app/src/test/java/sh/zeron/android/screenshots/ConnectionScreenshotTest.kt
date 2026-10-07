package sh.zeron.android.screenshots

import android.app.Application
import android.os.Looper
import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.compose.ui.test.longClick
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.onRoot
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTouchInput
import androidx.compose.ui.test.performScrollToNode
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
import sh.zeron.android.MainActivity
import sh.zeron.android.R
import sh.zeron.android.core.ConnectionState
import sh.zeron.android.core.ConnectionState.Dot
import sh.zeron.android.core.ConnectionState.Workspace
import sh.zeron.android.core.Machine
import sh.zeron.android.core.MachineStore
import sh.zeron.android.core.ZeronModel

/**
 * Home title bar connection chip (green / yellow / red, long name), the
 * quick switcher and the failure sheet, light + dark. The chip state is
 * pinned with ZeronModel.previewConnection over the Demo workspace.
 * Output: connection/ (English) or zh/connection/.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [34], qualifiers = "w411dp-h891dp-night-xxhdpi")
open class ConnectionScreenshotTest {
    protected open val subdir: String = ""

    @get:Rule
    val compose = createEmptyComposeRule()

    @Before
    fun gate() {
        Screenshots.assumeEnabled()
        Screenshots.assumeHostCore()
    }

    private val studio = Machine(id = "fixture-studio", name = "Studio PC", host = "192.168.1.20", user = "dev", hostKey = "SHA256:fixture")
    private val buildBox = Machine(id = "fixture-build", name = "Build box", host = "build.local", port = 2222, user = "ci", auth = Machine.AUTH_PASSWORD, hostKey = "SHA256:fixture")
    private val longName = "Workstation-Shanghai-Office-3F-RTX4090 (Windows 11 Pro)"

    @Test
    fun connectionScreens() {
        val app = ApplicationProvider.getApplicationContext<Application>()
        org.robolectric.shadows.ShadowBuild.setModel("Pixel 8")
        FakeAndroidKeyStore.install()
        app.getSharedPreferences("zeron-update", 0).edit().putLong("lastCheck", System.currentTimeMillis()).commit()
        MachineStore(app).apply {
            save(studio, null)
            save(buildBox, null)
        }
        val scenario = ActivityScenario.launch(MainActivity::class.java)
        lateinit var model: ZeronModel
        scenario.onActivity { model = ViewModelProvider(it)[ZeronModel::class.java] }
        settleUntil("demo workspace") { model.phase == ZeronModel.Phase.Ready && model.workspace != null }
        model.applyListMode(ZeronModel.ListMode.Project)

        fun pin(dot: Dot, title: String = "Studio PC", error: String? = null, retryAtMs: Long? = null) {
            model.previewConnection = ConnectionState.View(title, Workspace.DIRECT, dot, error, retryAtMs, id = studio.id)
        }

        for ((mode, suffix) in listOf(2 to "dark", 1 to "light")) {
            model.applyAppearance(mode)
            model.connectionSheet = null
            pin(Dot.CONNECTED)
            settle()
            capture("connection/01-chip-connected-$suffix.png")
            pin(Dot.CONNECTING)
            settle()
            capture("connection/02-chip-connecting-$suffix.png")
            pin(Dot.FAILED, error = "timed out reaching 192.168.1.20:22")
            settle()
            capture("connection/03-chip-disconnected-$suffix.png")
            pin(Dot.CONNECTED, title = longName)
            settle()
            capture("connection/04-chip-long-name-$suffix.png")

            // Chip long-press -> quick switcher.
            pin(Dot.CONNECTED)
            settle()
            // Long-press: the quick switcher (a tap opens the computer page).
            compose.onNodeWithTag("connection-chip").performTouchInput { longClick() }
            settle()
            check(model.connectionSheet == ZeronModel.ConnectionSheet.SWITCHER)
            capture("connection/05-switcher-$suffix.png")
            model.connectionSheet = null

            // Failure sheet (timeout, automatic retry pending).
            pin(Dot.FAILED, error = "timed out reaching 192.168.1.20:22", retryAtMs = System.currentTimeMillis() + 12_500)
            settle()
            // The failure sheet pops by itself once per episode (a chip tap opens the computer page).
            model.connectionSheet = ZeronModel.ConnectionSheet.FAILURE
            settle()
            check(model.connectionSheet == ZeronModel.ConnectionSheet.FAILURE)
            capture("connection/06-failure-timeout-$suffix.png")
            model.connectionSheet = null
            settle()

            // Key rejected: needs the user, details expanded, Edit Computer link.
            pin(Dot.FAILED, error = "the machine rejected this phone's key for user dev (add the phone's public key to authorized_keys)")
            model.connectionSheet = ZeronModel.ConnectionSheet.FAILURE
            settle()
            compose.onNodeWithText(app.getString(R.string.conn_show_details)).performClick()
            settle()
            capture("connection/07-failure-auth-details-$suffix.png")
            model.connectionSheet = null
            settle()
        }
        model.applyAppearance(2)
        scenario.close()
    }

    /** The update badge beside the chip (arrow, ring, checkmark) and the Settings toggle. */
    @Test
    fun updateBadgeScreens() {
        val app = ApplicationProvider.getApplicationContext<Application>()
        org.robolectric.shadows.ShadowBuild.setModel("Pixel 8")
        FakeAndroidKeyStore.install()
        // No real probe or background download: the badge state is set by hand.
        app.getSharedPreferences("zeron-update", 0).edit().putLong("lastCheck", System.currentTimeMillis()).putBoolean("autoUpdate", false).commit()
        MachineStore(app).apply { save(studio, null) }
        val scenario = ActivityScenario.launch(MainActivity::class.java)
        lateinit var model: ZeronModel
        scenario.onActivity { model = ViewModelProvider(it)[ZeronModel::class.java] }
        settleUntil("demo workspace") { model.phase == ZeronModel.Phase.Ready && model.workspace != null }
        model.applyListMode(ZeronModel.ListMode.Project)
        settle()
        val release = sh.zeron.android.core.Updater.Release(
            "round5-9", "round5-9", "Fixture notes", sh.zeron.android.BuildConfig.VERSION_CODE.toLong() + 1,
            "zeron-android-round5-9.apk", "", "https://example.invalid/a.apk", 1, "", null,
        )
        val apk = java.io.File(app.cacheDir, "fixture-update.apk").apply { writeBytes(byteArrayOf(0)) }

        for ((mode, suffix) in listOf(2 to "dark", 1 to "light")) {
            model.applyAppearance(mode)
            for ((title, tag) in listOf("Studio PC" to "", longName to "-long-name")) {
                model.previewConnection = ConnectionState.View(title, Workspace.DIRECT, Dot.CONNECTED, id = studio.id)
                model.updateRelease = release
                model.downloadedApk = null
                model.updateProgress = null
                settle()
                check(model.updateBadge == ZeronModel.UpdateBadge.AVAILABLE)
                capture("update/01-badge-arrow$tag-$suffix.png")
                model.updateProgress = 0.42f
                settle()
                check(model.updateBadge == ZeronModel.UpdateBadge.DOWNLOADING)
                capture("update/02-badge-downloading$tag-$suffix.png")
                model.downloadedApk = apk
                model.updateProgress = 1f
                settle()
                check(model.updateBadge == ZeronModel.UpdateBadge.READY)
                capture("update/03-badge-ready$tag-$suffix.png")
            }
            model.updateRelease = null
            model.updateProgress = null
            model.downloadedApk = null
            model.tab = ZeronModel.Tab.Settings
            settle()
            compose.onNode(androidx.compose.ui.test.hasScrollToIndexAction())
                .performScrollToNode(androidx.compose.ui.test.hasText(app.getString(R.string.auto_update)))
            model.applyAutoUpdate(true)
            settle()
            capture("update/04-settings-auto-update-on-$suffix.png")
            model.applyAutoUpdate(false)
            settle()
            capture("update/05-settings-auto-update-off-$suffix.png")
            model.tab = ZeronModel.Tab.Sessions
            settle()
        }
        model.applyAppearance(2)
        scenario.close()
    }

    private fun capture(name: String) {
        compose.onRoot().captureRoboImage(Screenshots.path(subdir + name))
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
