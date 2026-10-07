package sh.zeron.android.ui

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
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
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
import sh.zeron.android.core.ZeronModel
import sh.zeron.android.screenshots.FakeAndroidKeyStore
import sh.zeron.android.screenshots.Screenshots
import uniffi.zeron_core.BusyPolicy
import uniffi.zeron_core.DebugEntry
import uniffi.zeron_core.LayoutListener
import uniffi.zeron_core.SendRequest
import uniffi.zeron_core.TranscriptView

/**
 * Session transcript controls: a code block's copy button copies the code
 * and shows its check for ~2s, then reverts; the message navigator appears
 * once there are two of your messages, opens on tap, and a pick glides the
 * transcript to that message.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34], qualifiers = "w411dp-h891dp-xxhdpi")
class TranscriptControlsTest {
    @get:Rule
    val compose = createEmptyComposeRule()

    /**
     * Needs the Rust transcript layout, whose text-measurer callback is
     * registered process-wide: after enough other Robolectric sandboxes in
     * one JVM it lands in a stale one and the frame stays empty (or Compose
     * never idles), the same order dependence the screenshot suite has. So
     * it runs with the screenshot suite, class by class:
     * `-PzeronScreenshots=true --tests '*TranscriptControlsTest'`.
     */
    @Before
    fun gate() {
        Screenshots.assumeEnabled()
        Screenshots.assumeHostCore()
    }

    private lateinit var scenario: ActivityScenario<MainActivity>
    private lateinit var model: ZeronModel

    private fun launch(chat: String) {
        FakeAndroidKeyStore.install()
        scenario = ActivityScenario.launch(MainActivity::class.java)
        scenario.onActivity { model = ViewModelProvider(it)[ZeronModel::class.java] }
        val app = model.getApplication<Application>()
        app.getSharedPreferences("zeron-update", 0).edit().putLong("lastCheck", System.currentTimeMillis()).putBoolean("autoUpdate", false).commit()
        settleUntil("demo workspace") { model.phase == ZeronModel.Phase.Ready && model.workspace != null }
        // An earlier test in this JVM may have left the shared app on another
        // workspace (a machine, the cloud): these fixtures are the demo's.
        fun hasChat() = model.workspace?.let { w -> (w.front.pinned + w.front.sections.flatMap { s -> s.sessions } + w.front.recent).any { r -> r.id == chat } } == true
        if (model.activeMachine != "demo" || !hasChat()) {
            model.enterDemo()
            settleUntil("the demo workspace") { model.phase == ZeronModel.Phase.Ready && hasChat() }
        }
        model.openSession(chat)
        // The transcript has laid out: its code blocks and your message are in.
        settleUntil("the transcript") { (transcript()?.height ?: 0) > 0 && transcript()?.codeRowKeys()?.isNotEmpty() == true && transcript()!!.userMarkCount >= 1 }
        settle(1000)
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

    @Test
    fun copyButtonShowsACheckThenReverts() {
        launch("chat-zh")
        val view = transcript()!!
        val key = view.codeRowKeys().first()
        view.scrollToRow(key)
        glide(view)
        val center = view.copyButtonCenters().first()
        tap(view, center.x, center.y)
        settle(300)
        assertNotNull("the tapped block shows the check", view.copiedCode)
        val clip = model.getApplication<Application>().getSystemService(android.content.ClipboardManager::class.java).primaryClip
        assertTrue("code on the clipboard", (clip?.getItemAt(0)?.text ?: "").isNotEmpty())
        settle(2400)
        assertNull("the check reverts after ~2s", view.copiedCode)
        scenario.close()
    }

    @Test
    fun navigatorListsYourMessagesAndJumps() {
        launch("chat-zh")
        // One message of yours: no navigator.
        assertEquals(0, compose.onAllNodesWithTag("msg-nav").fetchSemanticsNodes().size)
        val handle = model.client!!.openSession("chat-zh")
        for (text in listOf("把第二个代码块改成异步版本", "再补一个单元测试")) {
            handle.send(SendRequest(text = text, attachments = emptyList(), worktree = null, busy = BusyPolicy.QUEUE))
            settle(1500)
        }
        settleUntil("the navigator") { compose.onAllNodesWithTag("msg-nav").fetchSemanticsNodes().isNotEmpty() }
        settleUntil("three of your messages") { (transcript()?.userMarkCount ?: 0) >= 3 }
        compose.onNodeWithTag("msg-nav").performClick()
        settle()
        val items = compose.onAllNodesWithTag("msg-nav-item").fetchSemanticsNodes().size
        assertTrue("one preview per message, got $items", items >= 3)
        // The first message: the transcript glides up, away from the bottom.
        compose.onAllNodesWithTag("msg-nav-item")[0].performClick()
        settle(300)
        glide(transcript()!!)
        assertEquals(0, compose.onAllNodesWithTag("msg-nav-card").fetchSemanticsNodes().size)
        val view = transcript()!!
        assertEquals("reading your first message", 0, view.activeUserMark)
        assertTrue("away from the bottom", view.distanceFromBottomPx() > 0f)
        scenario.close()
    }

    /**
     * Regression: entering a session showed the FIRST message whenever a
     * finger happened to rest on the still-empty transcript while the rows
     * streamed in — a held touch (tracking, not yet a drag) froze the empty
     * frame's scroll of 0, and with no later frame to correct it the view
     * stayed pinned at the top. A finger that never passed the drag slop is
     * not a scroll: frames must keep landing at the bottom.
     */
    @Test
    fun heldTouchDuringLoadStillLandsAtBottom() {
        launch("chat-zh")
        var view: TranscriptListView? = null
        scenario.onActivity { activity ->
            view = TranscriptListView(activity).also { v ->
                v.engine = TranscriptView(model.text!!, object : LayoutListener {
                    override fun frameReady(revision: ULong) {}
                })
            }
            (activity.window.decorView as ViewGroup).addView(
                view,
                activity.resources.displayMetrics.widthPixels,
                activity.resources.displayMetrics.heightPixels,
            )
        }
        settle(800)
        // Frame one is the empty attach snapshot; the rows are still coming.
        scenario.onActivity { view!!.onFrame() }
        val t = SystemClock.uptimeMillis()
        scenario.onActivity {
            view!!.dispatchTouchEvent(MotionEvent.obtain(t, t, MotionEvent.ACTION_DOWN, 500f, 1200f, 0))
        }
        scenario.onActivity {
            view!!.engine!!.setDebugEntries(
                (1..24).map { i ->
                    DebugEntry("m$i", i % 3 == 0, "第 $i 条消息的正文，用来把 transcript 撑得比屏幕高。\n\n`row-$i`", false)
                },
                false,
            )
        }
        settle(1000)
        scenario.onActivity { view!!.onFrame() }
        // Still holding: the fresh rows must already sit at the bottom.
        assertEquals("a held touch must not pin the first message", 0f, view!!.distanceFromBottomPx(), 1f)
        scenario.onActivity {
            view!!.dispatchTouchEvent(MotionEvent.obtain(t, t + 400, MotionEvent.ACTION_UP, 500f, 1200f, 0))
        }
        settle(300)
        assertEquals("released at the bottom", 0f, view!!.distanceFromBottomPx(), 1f)
        scenario.close()
    }

    /** Robolectric doesn't draw the AndroidView, so run its glide (computeScroll) by hand. */
    private fun glide(view: TranscriptListView) {
        repeat(30) {
            scenario.onActivity { view.computeScroll() }
            settle(40)
        }
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
            check(System.currentTimeMillis() < end) {
                val v = transcript()
                "timed out waiting for $what (view=${v != null} h=${v?.height} code=${v?.codeRowKeys()?.size} marks=${v?.userMarkCount} frame=${runCatching { v?.engine?.frame()?.let { f -> "${f.rowCount()}/${f.totalHeight()}" } }.let { it.getOrNull() ?: it.exceptionOrNull()?.toString() }} draft=${model.getApplication<Application>().getSharedPreferences("drafts", 0).all} phase=${model.phase} stack=${model.sessionStack.toList()})"
            }
            settle(200)
        }
    }
}
