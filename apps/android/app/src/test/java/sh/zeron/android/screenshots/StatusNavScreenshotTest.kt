package sh.zeron.android.screenshots

import android.app.Application
import android.os.Looper
import android.os.SystemClock
import android.view.MotionEvent
import android.view.View
import android.view.ViewGroup
import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.compose.ui.test.onAllNodesWithTag
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
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
import uniffi.zeron_core.BusyPolicy
import uniffi.zeron_core.SendRequest

/**
 * Row status glyphs, the code block copy button and the message navigator;
 * dark + light. Output: status-nav/ (English) or zh/status-nav/.
 *
 * 01 home: running (dot-matrix only), done (check) and failed (red dot) rows,
 *    with done / failed already SEEN (they used to vanish then).
 * 02/03 a code block's copy button, then its check right after a tap.
 * 04/05 a session with the navigator collapsed, then opened.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [34], qualifiers = "w411dp-h891dp-night-xxhdpi")
open class StatusNavScreenshotTest {
    protected open val subdir: String = ""

    @get:Rule
    val compose = createEmptyComposeRule()

    @Before
    fun gate() {
        Screenshots.assumeEnabled()
        Screenshots.assumeHostCore()
    }

    private lateinit var scenario: ActivityScenario<MainActivity>

    @OptIn(ExperimentalRoborazziApi::class)
    @Test
    fun statusAndNavigator() {
        org.robolectric.shadows.ShadowBuild.setModel("Pixel 8")
        FakeAndroidKeyStore.install()
        scenario = ActivityScenario.launch(MainActivity::class.java)
        lateinit var model: ZeronModel
        scenario.onActivity { model = ViewModelProvider(it)[ZeronModel::class.java] }
        model.getApplication<Application>().getSharedPreferences("zeron-update", 0).edit()
            .putLong("lastCheck", System.currentTimeMillis()).putBoolean("autoUpdate", false).commit()
        settleUntil("demo workspace") { model.phase == ZeronModel.Phase.Ready && model.workspace != null }
        model.applyListMode(ZeronModel.ListMode.Project)
        // Done and failed, already seen (as if read on the desktop).
        model.client!!.markSeen("chat-tabs")
        model.client!!.markSeen("chat-errored")
        settle(1000)

        fun shot(name: String) = captureScreenRoboImage(Screenshots.path("${subdir}status-nav/$name"))
        val modes = listOf(2 to "dark", 1 to "light")
        for ((mode, suffix) in modes) {
            model.applyAppearance(mode)
            settle(1200)
            shot("01-home-status-$suffix.png")
        }

        // Code block copy button, before and right after a tap.
        model.openSession("chat-zh")
        settleUntil("the transcript") { (transcript()?.codeRowKeys()?.isNotEmpty() == true) }
        settle(1000)
        val view = transcript()!!
        for ((mode, suffix) in modes) {
            model.applyAppearance(mode)
            settle(1200)
            view.scrollToRow(view.codeRowKeys().first())
            glide(view)
            shot("02-code-copy-$suffix.png")
            val c = view.copyButtonCenters().first()
            tap(view, c.x, c.y)
            settle(300)
            shot("03-code-copied-$suffix.png")
            settle(2400)
        }

        // Your messages: two more, then the navigator.
        val handle = model.client!!.openSession("chat-zh")
        for (text in listOf("把第二个代码块改成异步版本，顺便处理超时", "再补一个单元测试，覆盖空输入", "最后更新 README 里的示例")) {
            handle.send(SendRequest(text = text, attachments = emptyList(), worktree = null, busy = BusyPolicy.QUEUE))
            settleUntil("the reply") { model.workspace!!.let { w -> (w.front.pinned + w.front.sections.flatMap { it.sessions } + w.front.recent) }.first { it.id == "chat-zh" }.indicator != uniffi.zeron_core.ChatIndicator.WORKING }
            settle(600)
        }
        settleUntil("four of your messages") { view.userMarkCount >= 4 }
        for ((mode, suffix) in modes) {
            model.applyAppearance(mode)
            settle(1200)
            // Reading the second message: its tick is the bright one.
            view.scrollToRow(view.userMarkKey(1)!!)
            glide(view)
            shot("04-nav-collapsed-$suffix.png")
            compose.onNodeWithTag("msg-nav").performClick()
            settle(800)
            shot("05-nav-expanded-$suffix.png")
            compose.onNodeWithTag("msg-nav").performClick()
            settle(600)
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

    private fun tap(v: View, x: Float, y: Float) {
        val t = SystemClock.uptimeMillis()
        v.dispatchTouchEvent(MotionEvent.obtain(t, t, MotionEvent.ACTION_DOWN, x, y, 0))
        v.dispatchTouchEvent(MotionEvent.obtain(t, t + 40, MotionEvent.ACTION_UP, x, y, 0))
    }

    private fun glide(view: TranscriptListView) {
        repeat(30) {
            scenario.onActivity { view.computeScroll() }
            settle(40)
        }
        settle(400)
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
