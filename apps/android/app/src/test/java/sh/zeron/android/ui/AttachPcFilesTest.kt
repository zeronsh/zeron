package sh.zeron.android.ui

import android.app.Application
import android.os.Looper
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.test.hasSetTextAction
import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.compose.ui.test.onAllNodesWithTag
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTextInput
import androidx.lifecycle.ViewModelProvider
import androidx.test.core.app.ActivityScenario
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

/**
 * Composer + → Computer files: the folder browser opens in the session's
 * folder listing files too; picked files go into the draft as mention links
 * (workspace-relative) or, outside the folder, as absolute paths. Nothing is
 * uploaded.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34], qualifiers = "w411dp-h891dp-xxhdpi")
class AttachPcFilesTest {
    @get:Rule
    val compose = createEmptyComposeRule()

    @Before
    fun gate() = Screenshots.assumeHostCore()

    @Test
    fun pickFilesInsertsReferences() {
        FakeAndroidKeyStore.install()
        val scenario = ActivityScenario.launch(MainActivity::class.java)
        lateinit var model: ZeronModel
        scenario.onActivity { model = ViewModelProvider(it)[ZeronModel::class.java] }
        val app = model.getApplication<Application>()
        app.getSharedPreferences("zeron-update", 0).edit().putLong("lastCheck", System.currentTimeMillis()).putBoolean("autoUpdate", false).commit()
        settleUntil("demo workspace") { model.phase == ZeronModel.Phase.Ready && model.workspace != null }
        // An earlier test in this JVM may have left the shared app on another
        // workspace (a machine, the cloud): these fixtures are the demo's.
        fun hasChat() = model.workspace?.let { w -> (w.front.pinned + w.front.sections.flatMap { s -> s.sessions } + w.front.recent).any { r -> r.id == "chat-zh" } } == true
        if (model.activeMachine != "demo" || !hasChat()) {
            model.enterDemo()
            settleUntil("the demo workspace") { model.phase == ZeronModel.Phase.Ready && hasChat() }
        }
        model.openSession("chat-zh")
        settle(1500)
        compose.onNode(hasSetTextAction()).performTextInput("Look at")
        settle()
        compose.onNodeWithTag("composer-attach").performClick()
        settle()
        assertEquals(1, compose.onAllNodesWithText(app.getString(R.string.attach_phone_photos)).fetchSemanticsNodes().size)
        compose.onNodeWithText(app.getString(R.string.attach_pc_files)).performClick()
        settleUntil("the session folder's files") { compose.onAllNodesWithTag("browser-file").fetchSemanticsNodes().isNotEmpty() }
        // Starts in the session's folder: folders, then its files.
        compose.onNodeWithText("/Users/dev/zeron").assertExists()
        compose.onNodeWithText("docs").assertExists()
        compose.onNodeWithText("README.md").performClick()
        settle()
        compose.onNodeWithText("docs").performClick()
        settleUntil("docs") { compose.onAllNodesWithText("release.md").fetchSemanticsNodes().isNotEmpty() }
        compose.onNodeWithText("release.md").performClick()
        settle()
        // Outside the session folder: home's todo.md.
        compose.onNodeWithText(app.getString(R.string.home_folder)).performClick()
        settleUntil("home") { compose.onAllNodesWithText("todo.md").fetchSemanticsNodes().isNotEmpty() }
        compose.onNodeWithText("todo.md").performClick()
        settle()
        compose.onNodeWithText(app.resources.getQuantityString(R.plurals.pc_files_insert, 3, 3)).performClick()
        settle()
        val field = compose.onNode(hasSetTextAction()).fetchSemanticsNode().config[SemanticsProperties.EditableText].text
        assertEquals(
            "Look at [README.md](zeron-file:README.md) [release.md](zeron-file:docs/release.md) `/Users/dev/todo.md` ",
            field,
        )
        assertTrue(compose.onAllNodesWithTag("browser-file").fetchSemanticsNodes().isEmpty())
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
