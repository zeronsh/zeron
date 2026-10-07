package sh.zeron.android.screenshots

import android.app.Application
import android.os.Looper
import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.compose.ui.test.hasAnyAncestor
import androidx.compose.ui.test.hasTestTag
import androidx.compose.ui.test.onNodeWithTag
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
import sh.zeron.android.core.Endpoint
import sh.zeron.android.core.Machine
import sh.zeron.android.core.MachineStore
import sh.zeron.android.core.ZeronModel
import uniffi.zeron_core.DirectEndpointStat
import uniffi.zeron_core.DirectPhase
import uniffi.zeron_core.DirectStatus

/**
 * The computer page's 地址 (Addresses) list: the route tag of the address the link runs
 * over right now is filled with the accent (white text); the others stay
 * plain. Dark + light, at home (LAN) and away (Tailscale), English here and
 * Chinese in the subclass. Output: connected-address/ (zh/connected-address/).
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [34], qualifiers = "w411dp-h891dp-night-xxhdpi")
open class ConnectedAddressScreenshotTest {
    protected open val subdir: String = ""

    @get:Rule
    val compose = createEmptyComposeRule()

    @Before
    fun gate() {
        Screenshots.assumeEnabled()
        Screenshots.assumeHostCore()
    }

    private val lan = Endpoint("192.168.1.102", 22)
    private val tailscale = Endpoint("100.124.7.39", 22)
    private val villa = Machine(
        id = "fixture-villa", name = "Villa", host = lan.host, user = "tx_vi", hostKey = "SHA256:fixture-villa",
        endpoints = listOf(lan, tailscale),
    )

    private fun stat(e: Endpoint, active: Boolean): DirectEndpointStat {
        val now = System.currentTimeMillis()
        return DirectEndpointStat(
            host = e.host, port = e.port.toUShort(), kind = e.kind.wire, active = active,
            lastAttemptMs = now - 95_000, lastOkMs = if (active) now - 95_000 else null,
            lastError = if (active) null else "timed out reaching ${e.host}:22", latencyMs = if (active) 12u else null,
        )
    }

    private fun live(active: Endpoint): DirectStatus {
        val now = System.currentTimeMillis()
        return DirectStatus(
            phase = DirectPhase.LIVE, lastError = null, retryAtMs = null,
            engineVersion = "0.2.100", engineDeviceId = "a41c07f2e9b3", notice = null,
            connectedAtMs = now - 95_000, syncedAtMs = now - 2_000, streams = emptyList(), log = emptyList(),
            clockOffsetMs = 0, endpoints = listOf(stat(lan, active == lan), stat(tailscale, active == tailscale)),
        )
    }

    @Test
    fun connectedAddressIsFilled() {
        val app = ApplicationProvider.getApplicationContext<Application>()
        org.robolectric.shadows.ShadowBuild.setModel("Pixel 8")
        FakeAndroidKeyStore.install()
        app.getSharedPreferences("zeron-update", 0).edit().putLong("lastCheck", System.currentTimeMillis()).putBoolean("autoUpdate", false).commit()
        MachineStore(app).save(villa, null)
        val scenario = ActivityScenario.launch(MainActivity::class.java)
        lateinit var model: ZeronModel
        scenario.onActivity { model = ViewModelProvider(it)[ZeronModel::class.java] }
        settleUntil("demo workspace") { model.phase == ZeronModel.Phase.Ready && model.workspace != null }

        for ((mode, suffix) in listOf(2 to "dark", 1 to "light")) {
            model.applyAppearance(mode)
            model.activeMachine = villa.id
            for ((active, name) in listOf(lan to "lan", tailscale to "tailscale")) {
                model.directStatus = live(active)
                assertEquals(active.key, model.connectedAddress(villa.id))
                model.editMachine = model.machines.first { it.id == villa.id }
                settle()
                compose.onNodeWithTag("addresses", useUnmergedTree = true).assertExists()
                // Exactly one filled tag, on the address in use; the other stays plain.
                fun inList(tag: String) = compose.onAllNodes(hasTestTag(tag) and hasAnyAncestor(hasTestTag("addresses")), useUnmergedTree = true)
                    .fetchSemanticsNodes().size
                assertEquals(1, inList("route-tag-connected"))
                assertEquals(1, inList("route-tag"))
                capture("connected-$name-$suffix.png")
                model.editMachine = null
                settle(300)
            }
            // Not connected: no filled tag.
            model.directStatus = live(lan).copy(phase = DirectPhase.FAILED)
            assertEquals(null, model.connectedAddress(villa.id))
        }
        model.directStatus = null
        model.activeMachine = "demo"
        model.applyAppearance(2)
        scenario.close()
    }

    private fun capture(name: String) {
        compose.onRoot().captureRoboImage(Screenshots.path("${subdir}connected-address/$name"))
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
class ConnectedAddressZhScreenshotTest : ConnectedAddressScreenshotTest() {
    override val subdir: String = "zh/"
}
