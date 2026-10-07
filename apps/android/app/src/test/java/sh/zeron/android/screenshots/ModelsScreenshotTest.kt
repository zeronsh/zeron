package sh.zeron.android.screenshots

import android.app.Application
import android.os.Looper
import androidx.compose.ui.test.junit4.createEmptyComposeRule
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
 * New Session's model menu: the computer's live Codex list (a custom
 * gpt-6.1-sol on top, as a real 0.2.100 engine reports it) and the fallback
 * when the live read failed (the saved list + 「没能从电脑读取最新列表 · 重试」 / "Couldn't get the latest list · Retry").
 * Output: models/ or zh/models/.
 */
@OptIn(ExperimentalRoborazziApi::class)
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [34], qualifiers = "w411dp-h891dp-night-xxhdpi")
open class ModelsScreenshotTest {
    protected open val subdir: String = ""

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
    fun modelMenus() {
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
        fun shot(name: String) = captureScreenRoboImage(Screenshots.path("${subdir}models/$name"))

        for ((mode, suffix) in listOf(2 to "dark", 1 to "light")) {
            model.applyAppearance(mode)
            // Live: the computer answered.
            CatalogHooks.models = { h, _ -> ModelCatalog(if (h == "codex") LIVE_CODEX else uniffi.zeron_core.fallbackModels(h), CatalogSource.LIVE, null) }
            openModelMenu(model)
            shot("01-live-clis-$suffix.png")
            compose.onNodeWithText("Codex").performClick()
            settle()
            shot("02-live-codex-$suffix.png")
            // Codex's traits: the reasoning ladder plus its service tier.
            compose.onNodeWithText("GPT-6.1-Sol").performClick()
            settle()
            compose.onNodeWithTag("chip-effort").performScrollTo().performClick()
            settle()
            shot("07-codex-traits-$suffix.png")
            compose.onNodeWithText("Fast").performClick()
            settle()
            compose.onNodeWithTag("chip-effort").performScrollTo()
            settle()
            shot("08-codex-fast-chip-$suffix.png")
            closeSheet(model)
            // Fallback: Codex's list was never read from this computer; the built-in one shows.
            CatalogHooks.models = { h, _ ->
                if (h == "codex") ModelCatalog(uniffi.zeron_core.fallbackModels(h), CatalogSource.STATIC, "Codex: not connected")
                else ModelCatalog(uniffi.zeron_core.fallbackModels(h), CatalogSource.LIVE, null)
            }
            openModelMenu(model)
            shot("03-fallback-clis-$suffix.png")
            compose.onNodeWithText("Codex").performClick()
            settle()
            shot("04-fallback-codex-$suffix.png")
            closeSheet(model)
        }
        model.applyAppearance(2)
        scenario.close()
    }

    /**
     * The project New Session opens on: the most recently used one (not the
     * first in the list) with nothing remembered, then the last pick on this
     * computer.
     */
    @Test
    fun defaultProject() {
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
        CatalogHooks.models = { h, _ -> ModelCatalog(uniffi.zeron_core.fallbackModels(h), CatalogSource.LIVE, null) }
        fun shot(name: String) = captureScreenRoboImage(Screenshots.path("${subdir}models/$name"))
        val projects = model.workspace!!.projects
        val recent = projects.maxBy { p -> p.sessions.maxOfOrNull { it.lastActivityMs } ?: p.createdAtMs }
        val other = projects.first { it.id != recent.id && it.id != projects.first().id }
        val memory = app.getSharedPreferences("zeron-new-session", 0)
        for ((mode, suffix) in listOf(2 to "dark", 1 to "light")) {
            model.applyAppearance(mode)
            memory.edit().clear().commit()
            model.showNewSession = true
            settle(1500)
            compose.onNodeWithTag("chip-project").performClick()
            settle()
            shot("05-project-most-recent-$suffix.png")
            closeSheet(model)
            memory.edit().clear().commit()
            sh.zeron.android.ui.NewSessionMemory.save(app, model.activeMachine, sh.zeron.android.ui.NewSessionMemory.Picks(other.id, null))
            model.showNewSession = true
            settle(1500)
            compose.onNodeWithTag("chip-project").performClick()
            settle()
            shot("06-project-remembered-$suffix.png")
            closeSheet(model)
        }
        memory.edit().clear().commit()
        model.applyAppearance(2)
        scenario.close()
    }

    private fun openModelMenu(model: ZeronModel) {
        model.showNewSession = true
        settle(1500)
        compose.onNodeWithTag("chip-model").performClick()
        settle()
    }

    private fun closeSheet(model: ZeronModel) {
        model.showNewSession = false
        settle()
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
        private val TIER = uniffi.zeron_core.ModelOption(
            "serviceTier", "Service Tier",
            listOf(uniffi.zeron_core.ModelOptionChoice("default", "Standard"), uniffi.zeron_core.ModelOptionChoice("fast", "Fast")),
            "default",
        )
        private fun m(id: String, label: String, levels: List<String>) = ModelInfo(id, label, null, levels, if (id == "gpt-5.2") emptyList() else listOf(TIER), null)
        private val SIX = listOf("low", "medium", "high", "xhigh", "max", "ultra")

        /** Codex as the user's computer reports it (ListModels capture, 0.2.100). */
        val LIVE_CODEX = listOf(
            m("gpt-6.1-sol", "GPT-6.1-Sol", SIX),
            m("gpt-6-astra", "GPT-6-Astra", SIX),
            m("gpt-5.6-sol", "GPT-5.6-Sol", SIX),
            m("gpt-5.6-terra", "GPT-5.6-Terra", SIX),
            m("gpt-5.6-luna", "GPT-5.6-Luna", SIX.dropLast(1)),
            m("gpt-5.5", "GPT-5.5", SIX.take(4)),
            m("gpt-5.2", "GPT-5.2", SIX.take(4)),
        )
    }
}
