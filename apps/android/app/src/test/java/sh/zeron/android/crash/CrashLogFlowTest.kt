package sh.zeron.android.crash

import android.app.Application
import android.content.ClipboardManager
import android.os.Looper
import androidx.compose.ui.test.hasScrollToIndexAction
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollToNode
import androidx.lifecycle.ViewModelProvider
import androidx.test.core.app.ActivityScenario
import androidx.test.core.app.ApplicationProvider
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config
import sh.zeron.android.MainActivity
import sh.zeron.android.R
import sh.zeron.android.core.CrashLog
import sh.zeron.android.core.ZeronModel
import sh.zeron.android.screenshots.FakeAndroidKeyStore
import sh.zeron.android.screenshots.Screenshots

/**
 * After a crash: the next launch's 上次意外退出 (Zeron Quit Unexpectedly) dialog copies the log and is
 * not shown again; Settings > About > Crash logs lists, and clears, them.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34], qualifiers = "w411dp-h891dp-xxhdpi")
class CrashLogFlowTest {
    @get:Rule
    val compose = createEmptyComposeRule()

    @Before
    fun gate() = Screenshots.assumeHostCore()

    @Test
    fun dialogCopiesThenSettingsListAndClear() {
        val app = ApplicationProvider.getApplicationContext<Application>()
        FakeAndroidKeyStore.install()
        app.getSharedPreferences("zeron-update", 0).edit().putLong("lastCheck", System.currentTimeMillis()).putBoolean("autoUpdate", false).commit()
        val scenario = ActivityScenario.launch(MainActivity::class.java)
        lateinit var model: ZeronModel
        scenario.onActivity { model = ViewModelProvider(it)[ZeronModel::class.java] }
        settleUntil("demo workspace") { model.phase == ZeronModel.Phase.Ready && model.workspace != null }
        // The model's own Application (Robolectric's shared factory can hold an
        // earlier test's): write the "previous run's" crash there and pick it up
        // as a fresh launch would.
        val ctx = model.getApplication<Application>()
        CrashLog.clear(ctx)
        CrashLog.write(ctx, "main", IllegalStateException("sync failed for 192.168.1.23"), 1_790_000_000_000L)
        model.lastCrash = CrashLog.pending(ctx)
        settle()

        compose.onNodeWithText(app.getString(R.string.crash_last_title)).assertExists()
        compose.onNodeWithTag("crash-copy").performClick()
        settle()
        assertNull(model.lastCrash)
        assertNull("seen: not shown on the next launch", CrashLog.pending(ctx))
        assertTrue(compose.onAllNodesWithText(app.getString(R.string.crash_last_title)).fetchSemanticsNodes().isEmpty())
        val clip = ctx.getSystemService(ClipboardManager::class.java).primaryClip!!.getItemAt(0).text.toString()
        assertTrue(clip, clip.startsWith("Zeron crash log\n"))
        assertTrue(clip, clip.contains("java.lang.IllegalStateException: sync failed for <ip>"))

        model.tab = ZeronModel.Tab.Settings
        settle()
        compose.onNode(hasScrollToIndexAction()).performScrollToNode(hasText(app.getString(R.string.crash_logs)))
        compose.onNodeWithText(app.resources.getQuantityString(R.plurals.crash_logs_count, 1, 1)).assertExists()
        compose.onNodeWithText(app.getString(R.string.crash_logs)).performClick()
        settle()
        assertTrue(model.showCrashLogs)
        compose.onNodeWithText("java.lang.IllegalStateException: sync failed for <ip>").assertExists()

        compose.onNodeWithText(app.getString(R.string.crash_logs_clear)).performClick()
        settle()
        compose.onAllNodesWithText(app.getString(R.string.crash_logs_clear))[1].performClick()
        settle()
        assertEquals(0, model.crashLogs.size)
        assertTrue(CrashLog.list(ctx).isEmpty())
        compose.onNodeWithText(app.getString(R.string.crash_logs_empty)).assertExists()
        model.showCrashLogs = false
        model.tab = ZeronModel.Tab.Sessions
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
