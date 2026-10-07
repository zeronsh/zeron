package sh.zeron.android.home

import android.app.Application
import android.os.Looper
import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.compose.ui.test.onAllNodesWithContentDescription
import androidx.compose.ui.test.onAllNodesWithText
import androidx.lifecycle.ViewModelProvider
import androidx.test.core.app.ActivityScenario
import androidx.test.core.app.ApplicationProvider
import androidx.compose.ui.graphics.Color
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import sh.zeron.android.design.MarkKind
import sh.zeron.android.ui.groupHeaderMark
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config
import sh.zeron.android.MainActivity
import sh.zeron.android.R
import sh.zeron.android.core.ZeronModel
import sh.zeron.android.screenshots.FakeAndroidKeyStore
import sh.zeron.android.screenshots.Screenshots
import uniffi.zeron_core.BusyPolicy
import uniffi.zeron_core.ChatIndicator
import uniffi.zeron_core.SendRequest

/**
 * The home list marks running sessions: a session with a turn in flight reads
 * WORKING in the snapshot and its row shows the dot-matrix alone (TalkBack: "Working"; no word
 * beside it), in both list views. Group headers show no
 * spinner (only an input dot when a row waits for the user).
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34], qualifiers = "w411dp-h891dp-xxhdpi")
class HomeRunningIndicatorTest {
    @get:Rule
    val compose = createEmptyComposeRule()

    @Before
    fun gate() = Screenshots.assumeHostCore()

    @Test
    fun runningSessionsShowWorking() {
        val app = ApplicationProvider.getApplicationContext<Application>()
        FakeAndroidKeyStore.install()
        app.getSharedPreferences("zeron-update", 0).edit().putLong("lastCheck", System.currentTimeMillis()).putBoolean("autoUpdate", false).commit()
        val scenario = ActivityScenario.launch(MainActivity::class.java)
        lateinit var model: ZeronModel
        scenario.onActivity { model = ViewModelProvider(it)[ZeronModel::class.java] }
        settleUntil("demo workspace") { model.phase == ZeronModel.Phase.Ready && model.workspace != null }
        val idle = allRows(model).first { it.indicator == ChatIndicator.IDLE }
        val handle = model.client!!.openSession(idle.id)
        handle.send(SendRequest(text = "Run the checks.", attachments = emptyList(), worktree = null, busy = BusyPolicy.QUEUE))
        settleUntil("the row to read WORKING") { allRows(model).any { it.id == idle.id && it.indicator == ChatIndicator.WORKING } }
        val working = app.getString(R.string.status_working)
        // Group headers carry no spinner: only an input dot, when a row waits.
        val rows = allRows(model)
        val workingOnly = rows.filter { it.indicator == ChatIndicator.WORKING }
        assertTrue(workingOnly.isNotEmpty())
        assertNull(groupHeaderMark(workingOnly + rows.filter { it.indicator == ChatIndicator.IDLE }, Color.Red))
        rows.firstOrNull { it.indicator == ChatIndicator.AWAITING_INPUT }?.let { waiting ->
            assertEquals(MarkKind.Dot(Color.Red), groupHeaderMark(workingOnly + waiting, Color.Red))
        }
        for (mode in listOf(ZeronModel.ListMode.Project, ZeronModel.ListMode.Activity)) {
            model.applyListMode(mode)
            settle()
            // Icon-only: the dot-matrix carries the state (TalkBack reads
            // "Working"); no "Working" word beside it.
            val shown = compose.onAllNodesWithContentDescription(working).fetchSemanticsNodes().size
            assertTrue("$mode: expected a \"$working\" status glyph, found $shown", shown >= 1)
            assertEquals("$mode: no \"$working\" text on rows", 0, compose.onAllNodesWithText(working).fetchSemanticsNodes().size)
        }
        scenario.close()
    }

    private fun allRows(model: ZeronModel) = model.workspace!!.let { it.front.pinned + it.front.sections.flatMap { s -> s.sessions } + it.front.recent }

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
