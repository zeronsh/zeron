package sh.zeron.android.screenshots

import android.app.Application
import android.os.Looper
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollTo
import androidx.lifecycle.ViewModelProvider
import androidx.test.core.app.ActivityScenario
import androidx.test.core.app.ApplicationProvider
import com.github.takahirom.roborazzi.ExperimentalRoborazziApi
import com.github.takahirom.roborazzi.captureScreenRoboImage
import org.junit.After
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
import sh.zeron.android.core.ZeronModel
import sh.zeron.android.ui.CatalogHooks
import uniffi.zeron_core.CatalogSource
import uniffi.zeron_core.ModelCatalog
import uniffi.zeron_core.ModelInfo

/**
 * New Session's model menu in Chinese, light theme, on the Demo computer:
 * Pi lists the same models from two providers (AG.20 and AG.50, same
 * names), so each row names its provider and the chip shows the picked
 * one's; the 「刷新模型列表」 ("Refresh model list") row re-reads the list,
 * showing the real error when that fails and the new list when it works.
 * Output: zh/model-providers-*.png.
 */
@OptIn(ExperimentalRoborazziApi::class)
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [34], qualifiers = "zh-rCN-w411dp-h891dp-notnight-xxhdpi")
class ModelProvidersZhScreenshotTest {
    @get:Rule
    val compose = createEmptyComposeRule()

    @Before
    fun gate() {
        Screenshots.assumeEnabled()
        Screenshots.assumeHostCore()
    }

    @After
    fun unhook() {
        CatalogHooks.models = null
    }

    @Test
    fun providersAndRefresh() {
        val app = ApplicationProvider.getApplicationContext<Application>()
        org.robolectric.shadows.ShadowBuild.setModel("Pixel 8")
        FakeAndroidKeyStore.install()
        app.getSharedPreferences("zeron-update", 0).edit().putLong("lastCheck", System.currentTimeMillis()).putBoolean("autoUpdate", false).commit()
        val scenario = ActivityScenario.launch(MainActivity::class.java)
        lateinit var model: ZeronModel
        scenario.onActivity { model = ViewModelProvider(it)[ZeronModel::class.java] }
        settleUntil("demo workspace") { model.phase == ZeronModel.Phase.Ready && model.workspace != null }
        if (model.activeMachine != "demo") {
            model.enterDemo()
            settleUntil("the demo") { model.phase == ZeronModel.Phase.Ready && model.activeMachine == "demo" }
        }
        model.applyAppearance(1)
        fun shot(name: String) = captureScreenRoboImage(Screenshots.path("zh/$name"))

        // The Demo computer's own Pi list (no hook): two providers, same names.
        model.showNewSession = true
        settle(1500)
        compose.onNodeWithTag("chip-model").performClick()
        settle()
        compose.onNodeWithText("Pi").performClick()
        settle()
        assertEquals(2, compose.onAllNodes(hasText("AG.20")).fetchSemanticsNodes().size)
        assertEquals(2, compose.onAllNodes(hasText("AG.50")).fetchSemanticsNodes().size)
        shot("model-providers-01-pi-list-light.png")
        // Pick AG.50's GPT-6 Astra: the chip and the CLI row name the provider.
        compose.onAllNodes(hasText("GPT-6 Astra").and(hasText("AG.50")))[0].performClick()
        settle()
        compose.onNodeWithText("GPT-6 Astra · AG.50").assertExists()
        // The chip row scrolls sideways: bring the whole model chip into view.
        compose.onNodeWithTag("chip-effort").performScrollTo()
        settle()
        shot("model-providers-02-chip-light.png")
        compose.onNodeWithTag("chip-model").performClick()
        settle()
        shot("model-providers-03-clis-light.png")

        // Refresh fails: the list stays, the real reason shows under the row.
        CatalogHooks.models = { h, force ->
            if (h == "pi" && force) ModelCatalog(PI, CatalogSource.SAVED, "host error: pi: provider AG.50 unreachable")
            else ModelCatalog(if (h == "pi") PI else uniffi.zeron_core.fallbackModels(h), CatalogSource.LIVE, null)
        }
        compose.onNodeWithText("Pi").performClick()
        settle()
        compose.onNodeWithText("刷新模型列表").performClick()
        settle()
        compose.onNodeWithText("刷新失败：pi: provider AG.50 unreachable").assertExists()
        assertEquals(2, compose.onAllNodes(hasText("AG.50")).fetchSemanticsNodes().size)
        settle(1500)
        shot("model-providers-04-refresh-failed-light.png")

        // Refresh works: the new list (a model added on the computer) replaces it.
        CatalogHooks.models = { h, _ ->
            ModelCatalog(if (h == "pi") PI + row("AG.50/gpt-6.2-nova", "GPT-6.2 Nova") else uniffi.zeron_core.fallbackModels(h), CatalogSource.LIVE, null)
        }
        compose.onNodeWithText("刷新模型列表").performClick()
        settle()
        compose.onNodeWithText("已从电脑更新").assertExists()
        compose.onNodeWithText("GPT-6.2 Nova").assertExists()
        settle(1500)
        shot("model-providers-05-refreshed-light.png")
        model.showNewSession = false
        settle()
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

    companion object {
        private val LADDER = listOf("minimal", "low", "medium", "high", "xhigh", "max")
        private fun row(id: String, label: String) = ModelInfo(id, label, null, LADDER, emptyList(), null)

        /** The Demo computer's Pi list (crates/client demo_models). */
        val PI = listOf(
            row("AG.20/gpt-6-astra", "GPT-6 Astra"),
            row("AG.20/gpt-6.1-sol", "GPT-6.1 Sol"),
            row("AG.50/gpt-6-astra", "GPT-6 Astra"),
            row("AG.50/gpt-6.1-sol", "GPT-6.1 Sol"),
        )
    }
}
