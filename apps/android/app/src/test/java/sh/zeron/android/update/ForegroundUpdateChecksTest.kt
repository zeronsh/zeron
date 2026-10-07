package sh.zeron.android.update

import android.app.Application
import android.os.Looper
import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.ViewModelProvider
import androidx.test.core.app.ActivityScenario
import androidx.test.core.app.ApplicationProvider
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config
import sh.zeron.android.MainActivity
import sh.zeron.android.core.ZeronModel
import sh.zeron.android.screenshots.FakeAndroidKeyStore
import sh.zeron.android.screenshots.Screenshots

/**
 * The periodic update check lives only while the app is in the foreground,
 * and 自动检查并下载更新 (Auto-check & download updates) off means no automatic check at all.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34])
class ForegroundUpdateChecksTest {
    @get:Rule
    val compose = createEmptyComposeRule()

    @Before
    fun gate() = Screenshots.assumeHostCore()

    @Test
    fun loopFollowsTheForegroundAndTheToggle() {
        val app = ApplicationProvider.getApplicationContext<Application>()
        FakeAndroidKeyStore.install()
        // Never checked, auto-update off: an automatic check would be due now.
        app.getSharedPreferences("zeron-update", 0).edit().clear().putBoolean("autoUpdate", false).commit()
        val scenario = ActivityScenario.launch(MainActivity::class.java)
        lateinit var model: ZeronModel
        scenario.onActivity { model = ViewModelProvider(it)[ZeronModel::class.java] }
        settleUntil("demo workspace") { model.phase == ZeronModel.Phase.Ready && model.workspace != null }
        settleUntil("start-up update check") { !model.updateCheckRunning }

        assertTrue("foreground check loop runs", model.foregroundChecksRunning)
        // Under Robolectric the shared ViewModel factory can hand the model an
        // earlier test's Application (and prefs): set state on the model's own.
        model.applyAutoUpdate(false)
        model.updater.lastCheckMs = 0
        model.updater.lastAttemptMs = 0

        scenario.moveToState(Lifecycle.State.CREATED)
        settle()
        assertFalse("background cancels the loop", model.foregroundChecksRunning)

        // Back in the foreground: the loop restarts and a check would be due
        // (never checked), but auto-update is off.
        scenario.moveToState(Lifecycle.State.RESUMED)
        settle()
        settleUntil("foreground update check") { !model.updateCheckRunning }
        assertTrue("back in the foreground restarts it", model.foregroundChecksRunning)
        assertEquals("toggle off: no automatic check", 0L, model.updater.lastAttemptMs)
        assertEquals(0L, model.updater.lastCheckMs)

        // Turning auto-update on right after a check: nothing due, no attempt.
        model.updater.lastCheckMs = System.currentTimeMillis()
        model.applyAutoUpdate(true)
        settleUntil("toggle update check") { !model.updateCheckRunning }
        assertEquals("checked a moment ago: not due", 0L, model.updater.lastAttemptMs)
        scenario.close()
    }

    private fun settle(ms: Long = 600) {
        val end = System.currentTimeMillis() + ms
        while (System.currentTimeMillis() < end) {
            shadowOf(Looper.getMainLooper()).idleFor(java.time.Duration.ofMillis(50))
            compose.mainClock.advanceTimeBy(50)
            Thread.sleep(20)
        }
        compose.waitForIdle()
    }

    private fun settleUntil(what: String, timeoutMs: Long = 30_000, done: () -> Boolean) {
        val end = System.currentTimeMillis() + timeoutMs
        while (!done()) {
            check(System.currentTimeMillis() < end) { "timed out waiting for $what" }
            settle(200)
        }
    }
}
