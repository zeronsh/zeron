package sh.zeron.android.screenshots

import android.app.Application
import android.os.Looper
import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.compose.ui.test.onRoot
import androidx.lifecycle.ViewModelProvider
import androidx.test.core.app.ActivityScenario
import androidx.test.core.app.ApplicationProvider
import com.github.takahirom.roborazzi.captureRoboImage
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
import sh.zeron.android.schedule.NewSessionSpec
import sh.zeron.android.schedule.ScheduledAlarms
import sh.zeron.android.schedule.ScheduledMessage

/**
 * A scheduled first message from the New Session screen: its chip above the
 * New Session composer and its row at the top of the home list; light + dark.
 * Output: schedule-new/ (English) or zh/schedule-new/.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [34], qualifiers = "w411dp-h891dp-night-xxhdpi")
open class ScheduledNewSessionScreenshotTest {
    protected open val subdir: String = ""

    @get:Rule
    val compose = createEmptyComposeRule()

    @Before
    fun gate() {
        Screenshots.assumeEnabled()
        Screenshots.assumeHostCore()
    }

    @Test
    fun scheduledNewSession() {
        val app = ApplicationProvider.getApplicationContext<Application>()
        org.robolectric.shadows.ShadowBuild.setModel("Pixel 8")
        FakeAndroidKeyStore.install()
        app.getSharedPreferences("zeron-update", 0).edit().putLong("lastCheck", System.currentTimeMillis()).putBoolean("autoUpdate", false).commit()
        val scenario = ActivityScenario.launch(MainActivity::class.java)
        lateinit var model: ZeronModel
        scenario.onActivity { model = ViewModelProvider(it)[ZeronModel::class.java] }
        settleUntil("demo workspace") { model.phase == ZeronModel.Phase.Ready && model.workspace != null }
        model.applyListMode(ZeronModel.ListMode.Project)
        val project = model.workspace!!.projects.first()
        val tomorrow = sh.zeron.android.schedule.ScheduleTime.next(9, 0, System.currentTimeMillis() + 86_400_000L - 60_000)
        ScheduledAlarms.schedule(
            app,
            ScheduledMessage(
                workspace = model.activeMachine,
                chatId = "",
                text = "Run the full test suite and summarise the failures.",
                atMs = System.currentTimeMillis() + 45 * 60_000L,
                newSession = NewSessionSpec(projectId = project.id, harness = "codex", label = project.name),
            ),
        )
        ScheduledAlarms.schedule(
            app,
            ScheduledMessage(
                workspace = model.activeMachine,
                chatId = "",
                text = "Morning triage: open issues labelled bug.",
                atMs = tomorrow,
                newSession = NewSessionSpec(projectId = project.id, harness = "claude-code", label = project.name),
            ),
        )
        for ((mode, suffix) in listOf(2 to "dark", 1 to "light")) {
            model.applyAppearance(mode)
            settle()
            capture("schedule-new/01-home-pending-$suffix.png")
            model.showNewSession = true
            settle(1500)
            capture("schedule-new/02-new-session-chip-$suffix.png")
            model.showNewSession = false
            settle()
        }
        model.applyAppearance(2)
        scenario.close()
    }

    private fun capture(name: String) {
        compose.onRoot().captureRoboImage(Screenshots.path(subdir + name))
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
