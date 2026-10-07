package sh.zeron.android.screenshots

import android.app.Application
import android.os.Looper
import android.view.View
import android.view.ViewGroup
import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.compose.ui.test.onAllNodesWithTag
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
import sh.zeron.android.core.ZeronModel
import sh.zeron.android.ui.TranscriptListView

/**
 * A file link tapped in a reply opens the file preview: a Markdown report
 * read from the chat's workspace (Demo's computer answers with a sample),
 * and a link outside the project folder. The tap goes through the
 * transcript's real `onLink`. Output: files/ or zh/files/.
 */
@OptIn(ExperimentalRoborazziApi::class)
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [34], qualifiers = "w411dp-h891dp-night-xxhdpi")
open class FilePreviewScreenshotTest {
    protected open val subdir: String = ""

    @get:Rule
    val compose = createEmptyComposeRule()

    @Before
    fun gate() {
        Screenshots.assumeEnabled()
        Screenshots.assumeHostCore()
    }

    private lateinit var scenario: ActivityScenario<MainActivity>

    @Test
    fun filePreview() {
        org.robolectric.shadows.ShadowBuild.setModel("Pixel 8")
        FakeAndroidKeyStore.install()
        scenario = ActivityScenario.launch(MainActivity::class.java)
        lateinit var model: ZeronModel
        scenario.onActivity { model = ViewModelProvider(it)[ZeronModel::class.java] }
        model.getApplication<Application>().getSharedPreferences("zeron-update", 0).edit()
            .putLong("lastCheck", System.currentTimeMillis()).putBoolean("autoUpdate", false).commit()
        settleUntil("demo workspace") { model.phase == ZeronModel.Phase.Ready && model.workspace != null }
        model.openSession("chat-zh")
        settleUntil("the transcript") { transcript() != null }
        settle(1000)
        fun shot(name: String) = captureScreenRoboImage(Screenshots.path("${subdir}files/$name"))
        fun tapLink(url: String) {
            val view = transcript()!!
            scenario.onActivity { view.onLink(url) }
            settleUntil("the preview") { compose.onAllNodesWithTag("file-preview").fetchSemanticsNodes().isNotEmpty() }
            settle(1200)
        }
        for ((mode, suffix) in listOf(2 to "dark", 1 to "light")) {
            model.applyAppearance(mode)
            settle(800)
            // As agents write it: a path with an editor line ref.
            tapLink("docs/传输层重构报告.md:1")
            shot("01-md-preview-$suffix.png")
            tapLink("src/main.rs:3")
            shot("02-text-preview-$suffix.png")
            tapLink("/etc/hosts")
            shot("03-outside-project-$suffix.png")
        }
        model.applyAppearance(2)
        scenario.close()
    }

    private fun transcript(): TranscriptListView? {
        var found: TranscriptListView? = null
        scenario.onActivity { found = find(it.window.decorView) }
        return found
    }

    private fun find(v: View): TranscriptListView? {
        if (v is TranscriptListView) return v
        if (v is ViewGroup) for (i in 0 until v.childCount) find(v.getChildAt(i))?.let { return it }
        return null
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

@Config(sdk = [34], qualifiers = "zh-rCN-w411dp-h891dp-night-xxhdpi")
class FilePreviewZhScreenshotTest : FilePreviewScreenshotTest() {
    override val subdir: String = "zh/"
}
