package sh.zeron.android.screenshots

import android.app.Application
import android.os.Looper
import androidx.compose.ui.test.hasScrollToIndexAction
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.compose.ui.test.onRoot
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollToNode
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
import sh.zeron.android.R
import sh.zeron.android.core.ZeronModel
import sh.zeron.android.design.AccentChoice
import sh.zeron.android.design.ZeronThemes

/**
 * Themes and accent colors: the Appearance settings (theme rows, accent swatches, the theme
 * picker), then home + a session in the default Zeron purple, the Orange and Blue accents,
 * and the Catppuccin themes; light + dark. Output: themes/ (English) or zh/themes/.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [34], qualifiers = "w411dp-h891dp-night-xxhdpi")
open class ThemeScreenshotTest {
    protected open val subdir: String = ""

    @get:Rule
    val compose = createEmptyComposeRule()

    @Before
    fun gate() {
        Screenshots.assumeEnabled()
        Screenshots.assumeHostCore()
    }

    @Test
    fun themes() {
        val app = ApplicationProvider.getApplicationContext<Application>()
        org.robolectric.shadows.ShadowBuild.setModel("Pixel 8")
        FakeAndroidKeyStore.install()
        app.getSharedPreferences("zeron-update", 0).edit().putLong("lastCheck", System.currentTimeMillis()).putBoolean("autoUpdate", false).commit()
        val scenario = ActivityScenario.launch(MainActivity::class.java)
        lateinit var model: ZeronModel
        scenario.onActivity { model = ViewModelProvider(it)[ZeronModel::class.java] }
        settleUntil("demo workspace") { model.phase == ZeronModel.Phase.Ready && model.workspace != null }
        model.applyListMode(ZeronModel.ListMode.Project)
        val ws = model.workspace!!
        val chat = (ws.front.pinned + ws.front.sections.flatMap { it.sessions } + ws.front.recent).first()

        // Settings > Appearance, default theme, both appearances; then the picker sheet.
        for ((mode, suffix) in listOf(2 to "dark", 1 to "light")) {
            model.applyAppearance(mode)
            model.tab = ZeronModel.Tab.Settings
            settle()
            compose.onNode(hasScrollToIndexAction()).performScrollToNode(hasText(app.getString(R.string.language_chinese)))
            settle()
            capture("themes/01-settings-appearance-$suffix.png")
            compose.onNodeWithText(app.getString(if (mode == 2) R.string.theme_dark else R.string.theme_light)).performClick()
            settle(1200)
            capture("themes/02-theme-picker-$suffix.png")
            scenario.onActivity { it.onBackPressedDispatcher.onBackPressed() }
            settle()
        }
        // Accent override shown in settings.
        model.applyAccent(AccentChoice.ORANGE)
        model.applyAppearance(2)
        settle()
        compose.onNode(hasScrollToIndexAction()).performScrollToNode(hasText(app.getString(R.string.language_chinese)))
        settle()
        capture("themes/03-settings-orange-dark.png")
        model.tab = ZeronModel.Tab.Sessions
        settle()

        val looks = listOf(
            Triple("zeron", Pair(ZeronThemes.DEFAULT_LIGHT, ZeronThemes.DEFAULT_DARK), AccentChoice.THEME),
            Triple("orange", Pair(ZeronThemes.DEFAULT_LIGHT, ZeronThemes.DEFAULT_DARK), AccentChoice.ORANGE),
            Triple("blue", Pair(ZeronThemes.DEFAULT_LIGHT, ZeronThemes.DEFAULT_DARK), AccentChoice.BLUE),
            Triple("catppuccin", Pair("catppuccin-latte", "catppuccin-mocha"), AccentChoice.THEME),
        )
        for ((name, ids, accent) in looks) {
            model.applyTheme(false, ids.first)
            model.applyTheme(true, ids.second)
            model.applyAccent(accent)
            for ((mode, suffix) in listOf(2 to "dark", 1 to "light")) {
                model.applyAppearance(mode)
                settle()
                capture("themes/10-home-$name-$suffix.png")
                model.openSession(chat.id)
                settle(2000)
                capture("themes/11-session-$name-$suffix.png")
                model.back()
                settle()
            }
        }
        model.applyTheme(false, ZeronThemes.DEFAULT_LIGHT)
        model.applyTheme(true, ZeronThemes.DEFAULT_DARK)
        model.applyAccent(AccentChoice.THEME)
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
