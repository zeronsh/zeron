package sh.zeron.android.screenshots

import android.app.Application
import android.os.Looper
import androidx.compose.ui.test.hasScrollToIndexAction
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollToNode
import androidx.lifecycle.ViewModelProvider
import androidx.test.core.app.ActivityScenario
import androidx.test.core.app.ApplicationProvider
import com.github.takahirom.roborazzi.ExperimentalRoborazziApi
import com.github.takahirom.roborazzi.captureScreenRoboImage
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
import sh.zeron.android.core.CrashLog
import sh.zeron.android.core.ZeronModel

/**
 * The local crash log: the next launch's dialog, the Settings > About row
 * and the Crash logs page (collapsed, then one log expanded); dark + light.
 * Output: crash/ (English) or zh/crash/.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [34], qualifiers = "w411dp-h891dp-night-xxhdpi")
open class CrashLogScreenshotTest {
    protected open val subdir: String = ""

    @get:Rule
    val compose = createEmptyComposeRule()

    @Before
    fun gate() {
        Screenshots.assumeEnabled()
        Screenshots.assumeHostCore()
    }

    private class SyncEngine {
        fun apply(rows: List<Int>, index: Int): Int = rows[index]
        fun refresh(host: String): Int = try {
            apply(listOf(1, 2, 3), 3)
        } catch (e: IndexOutOfBoundsException) {
            throw IllegalStateException("transcript refresh failed after reconnect to $host", e)
        }
    }

    @OptIn(ExperimentalRoborazziApi::class)
    @Test
    fun crashLog() {
        val app = ApplicationProvider.getApplicationContext<Application>()
        org.robolectric.shadows.ShadowBuild.setModel("Pixel 8")
        FakeAndroidKeyStore.install()
        app.getSharedPreferences("zeron-update", 0).edit().putLong("lastCheck", System.currentTimeMillis()).putBoolean("autoUpdate", false).commit()
        val scenario = ActivityScenario.launch(MainActivity::class.java)
        lateinit var model: ZeronModel
        scenario.onActivity { model = ViewModelProvider(it)[ZeronModel::class.java] }
        settleUntil("demo workspace") { model.phase == ZeronModel.Phase.Ready && model.workspace != null }

        val ctx = model.getApplication<Application>()
        CrashLog.clear(ctx)
        val older = runCatching { error("unreachable") }.exceptionOrNull() ?: Exception()
        CrashLog.write(ctx, "DefaultDispatcher-worker-3", IllegalArgumentException("unknown agent icon: gemini-cli", older), 1_790_740_000_000L)
        val crash = runCatching { SyncEngine().refresh("192.168.1.23") }.exceptionOrNull()!!
        CrashLog.write(ctx, "main", crash, 1_790_763_000_000L)

        for ((mode, suffix) in listOf(2 to "dark", 1 to "light")) {
            model.applyAppearance(mode)
            model.lastCrash = CrashLog.pending(ctx)
            model.tab = ZeronModel.Tab.Sessions
            settle(1200)
            captureScreenRoboImage(Screenshots.path("${subdir}crash/01-last-crash-dialog-$suffix.png"))
            model.lastCrash = null

            model.tab = ZeronModel.Tab.Settings
            settle()
            compose.onNode(hasScrollToIndexAction()).performScrollToNode(hasText(app.getString(R.string.crash_logs)))
            settle()
            captureScreenRoboImage(Screenshots.path("${subdir}crash/02-settings-about-$suffix.png"))

            compose.onNodeWithText(app.getString(R.string.crash_logs)).performClick()
            settle()
            captureScreenRoboImage(Screenshots.path("${subdir}crash/03-crash-logs-$suffix.png"))
            compose.onNodeWithText("java.lang.IllegalStateException: transcript refresh failed after reconnect to <ip>").performClick()
            settle()
            captureScreenRoboImage(Screenshots.path("${subdir}crash/04-crash-log-expanded-$suffix.png"))
            model.showCrashLogs = false
            settle()
        }
        model.applyAppearance(2)
        model.tab = ZeronModel.Tab.Sessions
        CrashLog.clear(ctx)
        scenario.close()
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
