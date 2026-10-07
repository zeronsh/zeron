package sh.zeron.android.screenshots

import android.app.Application
import android.os.Looper
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.compose.ui.test.onRoot
import androidx.lifecycle.ViewModelProvider
import androidx.test.core.app.ActivityScenario
import androidx.test.core.app.ApplicationProvider
import com.github.takahirom.roborazzi.captureRoboImage
import org.junit.Assert.assertEquals
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
import sh.zeron.android.ui.TranscriptListView
import uniffi.zeron_core.BusyPolicy
import uniffi.zeron_core.DebugEntry
import uniffi.zeron_core.SendRequest

/**
 * The session screen's turn status, in Chinese, light theme (Demo workspace
 * on the host build of the Rust core): while a turn runs only the
 * transcript's tail row shows it (dot-matrix + elapsed; no "运行中 · …" pill
 * above the composer); once it ends the transcript ends with a done check
 * and when it ended, which stays. Right after a send the tail goes straight
 * to working (no "✓ 完成" (Done) from the previous turn first). While only a
 * Direct transcript's newest rows are here, it's headed with
 * "正在加载更早的消息…" (Loading earlier messages…).
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [34], qualifiers = "zh-rCN-w411dp-h891dp-notnight-xxhdpi")
class SessionStatusZhScreenshotTest {
    @get:Rule
    val compose = createEmptyComposeRule()

    @Before
    fun gate() {
        Screenshots.assumeEnabled()
        Screenshots.assumeHostCore()
    }

    @Test
    fun runningThenDone() {
        val app = ApplicationProvider.getApplicationContext<Application>()
        org.robolectric.shadows.ShadowBuild.setModel("Pixel 8")
        FakeAndroidKeyStore.install()
        app.getSharedPreferences("zeron-update", 0).edit().putLong("lastCheck", System.currentTimeMillis()).commit()
        val scenario = ActivityScenario.launch(MainActivity::class.java)
        lateinit var model: ZeronModel
        scenario.onActivity { model = ViewModelProvider(it)[ZeronModel::class.java] }
        settleUntil("demo workspace") { model.phase == ZeronModel.Phase.Ready && model.workspace != null }
        model.applyAppearance(1)
        settle()

        val ws = model.workspace!!
        val chat = (ws.front.pinned + ws.front.sections.flatMap { it.sessions } + ws.front.recent)
            .first {
                // A settled chat with no open question, so the composer (where
                // the old pill sat) is on screen.
                it.indicator != uniffi.zeron_core.ChatIndicator.WORKING &&
                    it.indicator != uniffi.zeron_core.ChatIndicator.AWAITING_INPUT
            }
        model.openSession(chat.id)
        settle(2000)
        val handle = model.client!!.openSession(chat.id)
        handle.send(SendRequest(text = "给淡入淡出的时序加一个测试。", attachments = emptyList(), worktree = null, busy = BusyPolicy.QUEUE))
        // Just sent: the tail says working, not how the last turn ended.
        settle(100)
        capture("session-status-just-sent-light.png")
        settleUntil("a running turn", 20_000) { handle.composer().live.turnRunning }
        val since = System.currentTimeMillis()
        settleUntil("the timer to tick", 20_000) { System.currentTimeMillis() - since > 4_000 || !handle.composer().live.turnRunning }
        // The composer pill no longer repeats the running state.
        val working = app.getString(R.string.status_working)
        val writing = app.getString(R.string.status_writing)
        assertEquals(0, compose.onAllNodes(hasText(working, substring = true).or(hasText(writing, substring = true))).fetchSemanticsNodes().size)
        capture("session-status-running-light.png")

        settleUntil("the turn to end", 60_000) { !handle.composer().live.turnRunning }
        settle(2500)
        capture("session-status-done-light.png")
        scenario.close()
    }

    @Test
    fun failedTurnEndsTheTranscriptWithARedDot() {
        val app = ApplicationProvider.getApplicationContext<Application>()
        org.robolectric.shadows.ShadowBuild.setModel("Pixel 8")
        FakeAndroidKeyStore.install()
        app.getSharedPreferences("zeron-update", 0).edit().putLong("lastCheck", System.currentTimeMillis()).commit()
        val scenario = ActivityScenario.launch(MainActivity::class.java)
        lateinit var model: ZeronModel
        scenario.onActivity { model = ViewModelProvider(it)[ZeronModel::class.java] }
        settleUntil("demo workspace") { model.phase == ZeronModel.Phase.Ready && model.workspace != null }
        model.applyAppearance(1)
        settle()
        // The demo's errored chat (its last turn failed 33 minutes ago).
        model.openSession("chat-errored")
        settle(2500)
        capture("session-status-failed-light.png")
        scenario.close()
    }

    @Test
    fun olderRowsStillLoadingHeadTheTranscript() {
        val app = ApplicationProvider.getApplicationContext<Application>()
        org.robolectric.shadows.ShadowBuild.setModel("Pixel 8")
        FakeAndroidKeyStore.install()
        app.getSharedPreferences("zeron-update", 0).edit().putLong("lastCheck", System.currentTimeMillis()).commit()
        val scenario = ActivityScenario.launch(MainActivity::class.java)
        lateinit var model: ZeronModel
        lateinit var activity: MainActivity
        scenario.onActivity { model = ViewModelProvider(it)[ZeronModel::class.java]; activity = it }
        settleUntil("demo workspace") { model.phase == ZeronModel.Phase.Ready && model.workspace != null }
        model.applyAppearance(1)
        settle()
        model.openSession("chat-errored")
        settle(2500)
        // The demo has no Direct link (whose opening tail sets this): feed
        // the transcript view the newest rows and the head row directly.
        val list = findView(activity.window.decorView, TranscriptListView::class.java)!!
        val engine = list.engine!!
        engine.setDebugHistoryPending(1_843_200uL)
        engine.setDebugEntries(
            listOf(
                DebugEntry("u1", true, "把重连的退避上限改成 30 秒，然后跑一遍测试。", false),
                DebugEntry("a1", false, "已把 `MAX_BACKOFF` 改为 30 秒，并更新了 `reconnect_backs_off` 测试的断言。\n\n`cargo test -p zeron-client` 全部通过（214 个）。", false),
                DebugEntry("u2", true, "好，提交吧。", false),
                DebugEntry("a2", false, "已提交：`client: cap the reconnect backoff at 30 s`。", false),
            ),
            false,
        )
        settle(1500)
        capture("session-history-loading-light.png")
        scenario.close()
    }

    private fun <T : android.view.View> findView(root: android.view.View, type: Class<T>): T? {
        if (type.isInstance(root)) return type.cast(root)
        if (root is android.view.ViewGroup) {
            for (i in 0 until root.childCount) findView(root.getChildAt(i), type)?.let { return it }
        }
        return null
    }

    private fun capture(name: String) {
        compose.onRoot().captureRoboImage(Screenshots.path("zh/$name"))
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
