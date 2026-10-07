package sh.zeron.android.l10n

import android.app.Application
import android.content.res.Configuration
import android.os.Build
import android.os.LocaleList
import android.os.Looper
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.hasScrollToIndexAction
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.onLast
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollToNode
import androidx.lifecycle.ViewModelProvider
import androidx.test.core.app.ApplicationProvider
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertSame
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.android.controller.ActivityController
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import sh.zeron.android.MainActivity
import sh.zeron.android.R
import sh.zeron.android.core.AppLanguage
import sh.zeron.android.core.ZeronModel
import sh.zeron.android.screenshots.FakeAndroidKeyStore
import sh.zeron.android.screenshots.Screenshots

/**
 * Settings > Language switches in place: same activity instance (no
 * recreation, so no window flash and no lost screen state), and the UI
 * text follows. Below Android 13 AppCompat applies the change itself; on
 * 13+ the system delivers a configuration change, simulated here since
 * Robolectric's LocaleManager only stores the choice.
 *
 * Needs the host build of the Rust core (the demo workspace), like the
 * screenshot renders; skipped without it.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(qualifiers = "en-w411dp-h891dp-night-xxhdpi")
class LanguageSwitchTest {
    @get:Rule val compose = createEmptyComposeRule()

    private val app get() = ApplicationProvider.getApplicationContext<Application>()

    @Before fun gate() {
        Screenshots.assumeHostCore()
        FakeAndroidKeyStore.install()
        org.robolectric.shadows.ShadowBuild.setModel("Pixel 8")
        app.getSharedPreferences("zeron-update", 0).edit().putLong("lastCheck", System.currentTimeMillis()).commit()
    }

    @After fun reset() {
        runCatching { AppLanguage.apply(app, AppLanguage.SYSTEM) }
    }

    @Test @Config(sdk = [34]) fun switchesInPlaceOnAndroid13Plus() = switchLanguage()

    @Test @Config(sdk = [32]) fun switchesInPlaceBelowAndroid13() = switchLanguage()

    private fun switchLanguage() {
        val controller = Robolectric.buildActivity(MainActivity::class.java).setup()
        val activity = controller.get()
        val model = ViewModelProvider(activity)[ZeronModel::class.java]
        settleUntil("demo workspace") { model.phase == ZeronModel.Phase.Ready && model.workspace != null }
        model.tab = ZeronModel.Tab.Settings
        settle()
        compose.onNodeWithText("Settings").assertIsDisplayed()
        compose.onNode(hasScrollToIndexAction()).performScrollToNode(hasText("简体中文"))
        settle()

        choose(controller, "简体中文", "zh-CN")
        assertSame("activity was recreated", activity, controller.get())
        assertEquals(ZeronModel.Tab.Settings, model.tab)
        assertEquals(AppLanguage.CHINESE, AppLanguage.current(activity))
        // Still scrolled to the Language group (the list wasn't rebuilt from the top).
        compose.onNodeWithText("语言").assertIsDisplayed()
        compose.onAllNodesWithText("跟随系统").onLast().assertIsDisplayed() // Appearance and Language both have it

        choose(controller, "English", "en")
        assertSame("activity was recreated", activity, controller.get())
        compose.onNodeWithText("Language").assertIsDisplayed()
        compose.onAllNodesWithText("Follow System").onLast().assertIsDisplayed()
    }

    private fun choose(controller: ActivityController<MainActivity>, label: String, tag: String) {
        compose.onNodeWithText(label).performClick()
        settle()
        if (Build.VERSION.SDK_INT >= 33) {
            val config = Configuration(controller.get().resources.configuration).apply { setLocales(LocaleList.forLanguageTags(tag)) }
            controller.configurationChange(config)
            settle()
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

    private fun settleUntil(what: String, timeoutMs: Long = 60_000, done: () -> Boolean) {
        val end = System.currentTimeMillis() + timeoutMs
        while (!done()) {
            check(System.currentTimeMillis() < end) { "timed out waiting for $what" }
            settle(200)
        }
    }
}
