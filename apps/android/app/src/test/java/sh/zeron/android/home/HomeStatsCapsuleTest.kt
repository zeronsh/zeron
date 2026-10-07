package sh.zeron.android.home

import android.app.Application
import android.os.Looper
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.getOrNull
import androidx.compose.ui.test.SemanticsMatcher
import androidx.compose.ui.test.assert
import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.lifecycle.ViewModelProvider
import androidx.test.core.app.ActivityScenario
import androidx.test.core.app.ApplicationProvider
import org.junit.Assert.assertEquals
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
import sh.zeron.android.core.ZeronModel
import sh.zeron.android.screenshots.FakeAndroidKeyStore
import sh.zeron.android.screenshots.Screenshots
import sh.zeron.android.ui.HomeStats
import uniffi.zeron_core.ChatIndicator
import uniffi.zeron_core.SendState

/**
 * The home title capsule: running / failed counts of the connected computer's
 * front-page sessions (as the rows' corners read them), the TalkBack heading,
 * the popover that opens a session, and the muted Idle state.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34], qualifiers = "w411dp-h891dp-xxhdpi")
class HomeStatsCapsuleTest {
    @get:Rule
    val compose = createEmptyComposeRule()

    @Before
    fun gate() = Screenshots.assumeHostCore()

    private lateinit var app: Application
    private lateinit var model: ZeronModel
    private lateinit var scenario: ActivityScenario<MainActivity>

    private fun launch() {
        app = ApplicationProvider.getApplicationContext()
        FakeAndroidKeyStore.install()
        app.getSharedPreferences("zeron-update", 0).edit().putLong("lastCheck", System.currentTimeMillis()).putBoolean("autoUpdate", false).commit()
        scenario = ActivityScenario.launch(MainActivity::class.java)
        scenario.onActivity { model = ViewModelProvider(it)[ZeronModel::class.java] }
        settleUntil("demo workspace") { model.phase == ZeronModel.Phase.Ready && model.workspace != null }
    }

    @Test
    fun countsMatchTheRowCorners() {
        launch()
        val ws = model.workspace!!
        val stats = HomeStats.of(ws)
        // The demo fixture: one session mid-turn, one whose turn errored.
        assertEquals(listOf("chat-veil"), stats.running.map { it.id })
        assertEquals(listOf("chat-errored"), stats.failed.map { it.id })

        // A failed send reads "failed" even while working (the corner's precedence);
        // a row in two groups counts once; newest first.
        val front = ws.front
        val veil = front.pinned.first { it.id == "chat-veil" }
        val idle = (front.pinned + front.sections.flatMap { it.sessions } + front.recent).first { it.indicator == ChatIndicator.IDLE }
        fun edit(r: uniffi.zeron_core.SessionRow) = when (r.id) {
            veil.id -> r.copy(sendState = SendState.FAILED)
            idle.id -> r.copy(indicator = ChatIndicator.WORKING, lastActivityMs = Long.MAX_VALUE)
            else -> r
        }
        val edited = ws.copy(
            front = front.copy(
                pinned = front.pinned.map(::edit),
                sections = front.sections.map { sec -> sec.copy(sessions = sec.sessions.map(::edit)) },
                // The errored session a second time (it must count once).
                recent = front.recent.map(::edit) + (front.pinned + front.sections.flatMap { it.sessions }).first { it.id == "chat-errored" },
            ),
        )
        val e = HomeStats.of(edited)
        assertEquals(listOf(idle.id), e.running.map { it.id })
        assertEquals(setOf("chat-veil", "chat-errored"), e.failed.map { it.id }.toSet())
        assertEquals(e.failed.size, e.failed.distinctBy { it.id }.size)
        assertTrue(HomeStats.of(null).idle)
        scenario.close()
    }

    @Test
    fun capsuleHeadingPopoverAndIdle() {
        launch()
        // No big "Sessions" title any more; the capsule is the heading.
        assertTrue(compose.onAllNodesWithText(app.getString(R.string.sessions)).fetchSemanticsNodes().isEmpty())
        val sessions = app.getString(R.string.sessions)
        val summary = app.resources.getQuantityString(R.plurals.home_stats_running, 1, 1) + app.getString(R.string.home_stats_separator) +
            app.resources.getQuantityString(R.plurals.home_stats_failed, 1, 1)
        compose.onNodeWithTag("home-stats")
            .assert(SemanticsMatcher.keyIsDefined(SemanticsProperties.Heading))
            .assert(SemanticsMatcher("described") { it.config.getOrNull(SemanticsProperties.ContentDescription)?.firstOrNull() == app.getString(R.string.home_stats_a11y, sessions, summary) })

        fun count(text: String) = compose.onAllNodesWithText(text).fetchSemanticsNodes().size
        val working = app.getString(R.string.status_working)
        val failed = app.getString(R.string.status_failed)
        val before = count(working) to count(failed)
        compose.onNodeWithTag("home-stats").performClick()
        settle()
        // Both group titles appear on top of the rows' own corners.
        assertEquals(before.first + 1, count(working))
        assertEquals(before.second + 1, count(failed))
        val errored = model.workspace!!.front.let { it.pinned + it.sections.flatMap { s -> s.sessions } + it.front_recent() }.first { it.id == "chat-errored" }
        // The popover row (the list row with the same title sits under it).
        compose.onAllNodesWithText(errored.title).fetchSemanticsNodes().let { assertTrue(it.size >= 2) }
        compose.onAllNodesWithText(errored.title)[compose.onAllNodesWithText(errored.title).fetchSemanticsNodes().size - 1].performClick()
        settleUntil("the failed session to open") { model.sessionStack.lastOrNull() == ZeronModel.Route.Session("chat-errored") }
        model.back()
        settle()

        // Nothing running or failed: a muted Idle capsule that opens nothing.
        model.archive("chat-veil")
        model.archive("chat-errored")
        settleUntil("both archived") { HomeStats.of(model.workspace).idle }
        settle()
        compose.onNodeWithText(app.getString(R.string.home_stats_idle), useUnmergedTree = true).assertExists()
        compose.onNodeWithTag("home-stats")
            .assert(SemanticsMatcher.keyIsDefined(SemanticsProperties.Heading))
            .assert(SemanticsMatcher.keyNotDefined(androidx.compose.ui.semantics.SemanticsActions.OnClick))
        scenario.close()
    }

    private fun uniffi.zeron_core.FrontPage.front_recent() = recent

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
