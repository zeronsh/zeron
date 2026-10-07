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
import sh.zeron.android.MainActivity
import sh.zeron.android.core.ZeronModel
import sh.zeron.android.design.AccentChoice
import sh.zeron.android.design.ZeronThemes
import uniffi.zeron_core.BusyPolicy
import uniffi.zeron_core.ChatIndicator
import uniffi.zeron_core.SendRequest

/**
 * Home list with running sessions (spinner + 运行中 (Working) corners on the rows;
 * group headers stay plain), in Chinese: Zeron (default) and Catppuccin, dark + light, By
 * Project and By Activity. Output: home-running/ under the renders dir.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [34], qualifiers = "zh-rCN-w411dp-h891dp-night-xxhdpi")
class HomeRunningScreenshotTest {
    @get:Rule
    val compose = createEmptyComposeRule()

    @Before
    fun gate() {
        Screenshots.assumeEnabled()
        Screenshots.assumeHostCore()
    }

    @Test
    fun homeWithRunningSessions() {
        val app = ApplicationProvider.getApplicationContext<Application>()
        org.robolectric.shadows.ShadowBuild.setModel("Pixel 8")
        FakeAndroidKeyStore.install()
        app.getSharedPreferences("zeron-update", 0).edit().putLong("lastCheck", System.currentTimeMillis()).putBoolean("autoUpdate", false).commit()
        val scenario = ActivityScenario.launch(MainActivity::class.java)
        lateinit var model: ZeronModel
        scenario.onActivity { model = ViewModelProvider(it)[ZeronModel::class.java] }
        settleUntil("demo workspace") { model.phase == ZeronModel.Phase.Ready && model.workspace != null }
        // Two more running turns beside the demo's own.
        val idle = rows(model).filter { it.indicator == ChatIndicator.IDLE }.take(2)
        val handles = idle.map { row ->
            model.client!!.openSession(row.id).also {
                it.send(SendRequest(text = "Keep going.", attachments = emptyList(), worktree = null, busy = BusyPolicy.QUEUE))
            }
        }
        settleUntil("running rows") { rows(model).count { it.indicator == ChatIndicator.WORKING } >= 2 }
        for ((name, ids) in listOf("zeron" to (ZeronThemes.DEFAULT_LIGHT to ZeronThemes.DEFAULT_DARK), "catppuccin" to ("catppuccin-latte" to "catppuccin-mocha"))) {
            model.applyTheme(false, ids.first)
            model.applyTheme(true, ids.second)
            model.applyAccent(AccentChoice.THEME)
            for ((mode, suffix) in listOf(2 to "dark", 1 to "light")) {
                model.applyAppearance(mode)
                model.applyListMode(ZeronModel.ListMode.Project)
                settle()
                capture("home-running/$name-by-project-$suffix.png")
                model.applyListMode(ZeronModel.ListMode.Activity)
                settle()
                capture("home-running/$name-by-activity-$suffix.png")
            }
        }
        handles.size
        model.applyTheme(false, ZeronThemes.DEFAULT_LIGHT)
        model.applyTheme(true, ZeronThemes.DEFAULT_DARK)
        model.applyListMode(ZeronModel.ListMode.Project)
        model.applyAppearance(2)
        scenario.close()
    }

    private fun rows(model: ZeronModel) = model.workspace!!.let { it.front.pinned + it.front.sections.flatMap { s -> s.sessions } + it.front.recent }

    private fun capture(name: String) {
        compose.onRoot().captureRoboImage(Screenshots.path(name))
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
