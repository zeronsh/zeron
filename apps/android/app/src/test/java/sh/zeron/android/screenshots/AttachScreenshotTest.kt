package sh.zeron.android.screenshots

import android.app.Application
import android.os.Looper
import androidx.compose.ui.test.hasSetTextAction
import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.compose.ui.test.onAllNodesWithTag
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTextClearance
import androidx.compose.ui.test.performTextInput
import androidx.lifecycle.ViewModelProvider
import androidx.test.core.app.ActivityScenario
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
import sh.zeron.android.core.ZeronModel

/**
 * Composer + → Phone photos / Computer files: the attach menu, the file
 * browser in the session's folder with two files selected, and the draft
 * with the inserted references; dark + light. Output: attach/ or zh/attach/.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [34], qualifiers = "w411dp-h891dp-night-xxhdpi")
open class AttachScreenshotTest {
    protected open val subdir: String = ""

    @get:Rule
    val compose = createEmptyComposeRule()

    @Before
    fun gate() {
        Screenshots.assumeEnabled()
        Screenshots.assumeHostCore()
    }

    @OptIn(ExperimentalRoborazziApi::class)
    @Test
    fun attachPcFiles() {
        org.robolectric.shadows.ShadowBuild.setModel("Pixel 8")
        FakeAndroidKeyStore.install()
        val scenario = ActivityScenario.launch(MainActivity::class.java)
        lateinit var model: ZeronModel
        scenario.onActivity { model = ViewModelProvider(it)[ZeronModel::class.java] }
        val app = model.getApplication<Application>()
        app.getSharedPreferences("zeron-update", 0).edit().putLong("lastCheck", System.currentTimeMillis()).putBoolean("autoUpdate", false).commit()
        settleUntil("demo workspace") { model.phase == ZeronModel.Phase.Ready && model.workspace != null }
        model.openSession("chat-zh")
        settle(1500)
        fun shot(name: String) = captureScreenRoboImage(Screenshots.path("${subdir}attach/$name"))
        val prompt = if (app.resources.configuration.locales[0].language == "zh") "看一下这两个文件" else "Take a look at"
        for ((mode, suffix) in listOf(2 to "dark", 1 to "light")) {
            model.applyAppearance(mode)
            settle(1200)
            compose.onNode(hasSetTextAction()).performTextClearance()
            compose.onNode(hasSetTextAction()).performTextInput(prompt)
            settle()
            compose.onNodeWithTag("composer-attach").performClick()
            settle(800)
            shot("01-attach-menu-$suffix.png")
            compose.onNodeWithText(app.getString(R.string.attach_pc_files)).performClick()
            settleUntil("files") { compose.onAllNodesWithTag("browser-file").fetchSemanticsNodes().isNotEmpty() }
            compose.onNodeWithText("README.md").performClick()
            compose.onNodeWithText("CHANGELOG.md").performClick()
            settle(800)
            shot("02-file-browser-$suffix.png")
            compose.onNodeWithText(app.resources.getQuantityString(R.plurals.pc_files_insert, 2, 2)).performClick()
            settle(1000)
            shot("03-composer-reference-$suffix.png")
        }
        model.applyAppearance(2)
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

    private fun settleUntil(what: String, timeoutMs: Long = 60_000, done: () -> Boolean) {
        val end = System.currentTimeMillis() + timeoutMs
        while (!done()) {
            check(System.currentTimeMillis() < end) { "timed out waiting for $what" }
            settle(200)
        }
    }
}
