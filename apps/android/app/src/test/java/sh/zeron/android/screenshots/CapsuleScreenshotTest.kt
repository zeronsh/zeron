package sh.zeron.android.screenshots

import android.app.Application
import android.os.Looper
import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import androidx.lifecycle.ViewModelProvider
import androidx.test.core.app.ActivityScenario
import androidx.test.core.app.ApplicationProvider
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
import sh.zeron.android.core.ConnectionState
import sh.zeron.android.core.ZeronModel
import sh.zeron.android.ui.HomeStats
import uniffi.zeron_core.BusyPolicy
import uniffi.zeron_core.ChatIndicator
import uniffi.zeron_core.SendRequest

/**
 * The home title's running / failed capsule: home with sessions running and
 * one failed, its popover, a long computer name beside it, then idle; dark +
 * light. Output: capsule/ (English) or zh/capsule/.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [34], qualifiers = "w411dp-h891dp-night-xxhdpi")
open class CapsuleScreenshotTest {
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
    fun capsule() {
        val app = ApplicationProvider.getApplicationContext<Application>()
        org.robolectric.shadows.ShadowBuild.setModel("Pixel 8")
        FakeAndroidKeyStore.install()
        app.getSharedPreferences("zeron-update", 0).edit().putLong("lastCheck", System.currentTimeMillis()).putBoolean("autoUpdate", false).commit()
        val scenario = ActivityScenario.launch(MainActivity::class.java)
        lateinit var model: ZeronModel
        scenario.onActivity { model = ViewModelProvider(it)[ZeronModel::class.java] }
        settleUntil("demo workspace") { model.phase == ZeronModel.Phase.Ready && model.workspace != null }
        model.applyListMode(ZeronModel.ListMode.Project)
        fun rows() = model.workspace!!.front.let { it.pinned + it.sections.flatMap { s -> s.sessions } + it.recent }

        // A second running session beside the fixture's running and errored ones.
        val idle = rows().first { it.indicator == ChatIndicator.IDLE && it.project != null }
        model.client!!.openSession(idle.id).send(SendRequest(text = "Run the checks.", attachments = emptyList(), worktree = null, busy = BusyPolicy.QUEUE))
        settleUntil("two running") { HomeStats.of(model.workspace).running.size >= 2 }

        fun shot(name: String) = captureScreenRoboImage(Screenshots.path("${subdir}capsule/$name"))
        for ((mode, suffix) in listOf(2 to "dark", 1 to "light")) {
            model.applyAppearance(mode)
            settle(1200)
            shot("01-home-running-failed-$suffix.png")
            compose.onNodeWithTag("home-stats").performClick()
            settle(1000)
            shot("02-sheet-$suffix.png")
            scenario.onActivity { it.onBackPressedDispatcher.onBackPressed() }
            settle()
            // A long computer name: the chip gets at least the room the old title left it.
            model.previewConnection = ConnectionState.View("Okhlv's MacBook Pro (Studio)", ConnectionState.Workspace.DIRECT, ConnectionState.Dot.CONNECTED, id = "studio")
            settle()
            shot("03-home-long-computer-name-$suffix.png")
            model.previewConnection = null
            settle()
        }

        // Idle: nothing running or failed on this computer.
        HomeStats.of(model.workspace).let { s -> (s.running + s.failed).forEach { model.archive(it.id) } }
        settleUntil("idle") { HomeStats.of(model.workspace).idle }
        for ((mode, suffix) in listOf(2 to "dark", 1 to "light")) {
            model.applyAppearance(mode)
            settle(1200)
            shot("04-home-idle-$suffix.png")
        }
        model.applyAppearance(2)
        scenario.close()
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
