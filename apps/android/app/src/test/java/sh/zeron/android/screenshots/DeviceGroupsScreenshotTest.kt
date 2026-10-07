package sh.zeron.android.screenshots

import android.app.Application
import android.os.Looper
import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.compose.ui.test.longClick
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.onRoot
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTouchInput
import androidx.compose.ui.test.performScrollTo
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
import sh.zeron.android.core.ConnectionState
import sh.zeron.android.core.ConnectionState.Dot
import sh.zeron.android.core.Endpoint
import sh.zeron.android.core.EndpointKind
import sh.zeron.android.core.Machine
import sh.zeron.android.core.MachineStore
import sh.zeron.android.core.NetworkSnapshot
import sh.zeron.android.core.Subnet
import sh.zeron.android.core.ConnectionState.Workspace
import sh.zeron.android.core.ZeronModel
import uniffi.zeron_core.DirectEndpointStat
import uniffi.zeron_core.DirectLogLine
import uniffi.zeron_core.DirectPhase
import uniffi.zeron_core.DirectStatus
import uniffi.zeron_core.DirectStreamStat

/**
 * One computer, several addresses (LAN + Tailscale), in Chinese, dark +
 * light: the chip's route tag, the switcher, Edit computer's address list
 * (and the merge picker), Connection details' 线路 (Routes) section, and the failure
 * sheet's per-address reasons. Output: device-groups/ under the renders dir.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [34], qualifiers = "zh-rCN-w411dp-h891dp-night-xxhdpi")
class DeviceGroupsScreenshotTest {
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
    private val laptop = Machine(
        id = "fixture-laptop", name = "MacBook", host = "100.88.21.5", user = "dev", hostKey = "SHA256:fixture-laptop",
        endpoints = listOf(Endpoint("100.88.21.5", 22)),
    )

    private val home = NetworkSnapshot(NetworkSnapshot.Transport.WIFI, listOf(Subnet.of("192.168.1.0", 24)!!), NetworkSnapshot.Vpn.NONE)
    private val outside = NetworkSnapshot(NetworkSnapshot.Transport.CELLULAR, emptyList(), NetworkSnapshot.Vpn.TAILSCALE)

    private fun stat(e: Endpoint, active: Boolean = false, ago: Long? = null, okAgo: Long? = null, error: String? = null, latency: Long? = null): DirectEndpointStat {
        val now = System.currentTimeMillis()
        return DirectEndpointStat(
            host = e.host, port = e.port.toUShort(), kind = e.kind.wire, active = active,
            lastAttemptMs = ago?.let { now - it }, lastOkMs = okAgo?.let { now - it }, lastError = error, latencyMs = latency?.toULong(),
        )
    }

    private fun status(phase: DirectPhase, endpoints: List<DirectEndpointStat>, error: String? = null, retryAtMs: Long? = null): DirectStatus {
        val now = System.currentTimeMillis()
        val live = phase == DirectPhase.LIVE
        return DirectStatus(
            phase = phase, lastError = error, retryAtMs = retryAtMs,
            engineVersion = if (live) "0.9.4" else null, engineDeviceId = if (live) "a41c07f2e9b3" else null, notice = null,
            connectedAtMs = if (live) now - 95_000 else null, syncedAtMs = if (live) now - 2_000 else null,
            streams = if (live) listOf(
                DirectStreamStat("sessions", 412u, 38u, 0u, 0u, now - 2_000, null),
                DirectStreamStat("projects", 57u, 9u, 0u, 0u, now - 40_000, null),
            ) else emptyList(),
            log = listOf(
                DirectLogLine(now - 97_000, "dialing 100.124.7.39:22 (tailscale)"),
                DirectLogLine(now - 95_500, "connected via 100.124.7.39:22 in 212 ms"),
            ),
            clockOffsetMs = if (live) 400 else null,
            endpoints = endpoints,
        )
    }

    @Test
    fun deviceGroupScreens() {
        val app = ApplicationProvider.getApplicationContext<Application>()
        org.robolectric.shadows.ShadowBuild.setModel("Pixel 8")
        FakeAndroidKeyStore.install()
        app.getSharedPreferences("zeron-update", 0).edit().putLong("lastCheck", System.currentTimeMillis()).putBoolean("autoUpdate", false).commit()
        MachineStore(app).apply {
            save(villa, null)
            save(laptop, null)
        }
        val scenario = ActivityScenario.launch(MainActivity::class.java)
        lateinit var model: ZeronModel
        scenario.onActivity { model = ViewModelProvider(it)[ZeronModel::class.java] }
        settleUntil("demo workspace") { model.phase == ZeronModel.Phase.Ready && model.workspace != null }
        model.applyListMode(ZeronModel.ListMode.Project)
        check(model.machines.first { it.id == villa.id }.addresses() == listOf(lan, tailscale))
        if (!model.autoRoute) model.applyAutoRoute(true)

        fun pin(dot: Dot, route: EndpointKind?, error: String? = null, retryAtMs: Long? = null) {
            model.previewConnection = ConnectionState.View("Villa", Workspace.DIRECT, dot, error, retryAtMs, id = villa.id, route = route)
        }
        val atHome = listOf(stat(lan, active = true, ago = 95_000, okAgo = 95_000, latency = 9), stat(tailscale))
        val away = listOf(
            stat(lan, ago = 97_000, error = "timed out reaching 192.168.1.102:22"),
            stat(tailscale, active = true, ago = 97_000, okAgo = 95_500, latency = 212),
        )
        val down = listOf(
            stat(lan, ago = 8_000, error = "timed out reaching 192.168.1.102:22"),
            stat(tailscale, ago = 8_000, error = "no route to host (os error 113) reaching 100.124.7.39:22"),
        )

        for ((mode, suffix) in listOf(2 to "dark", 1 to "light")) {
            model.applyAppearance(mode)
            model.connectionSheet = null
            model.activeMachine = villa.id

            // At home on Wi-Fi: LAN.
            model.network = home
            model.directStatus = status(DirectPhase.LIVE, atHome)
            pin(Dot.CONNECTED, EndpointKind.LAN)
            settle()
            compose.onNodeWithTag("route-tag", useUnmergedTree = true).assertExists()
            capture("01-chip-lan-$suffix.png")

            // Out on mobile data with Tailscale.
            model.network = outside
            model.directStatus = status(DirectPhase.LIVE, away)
            pin(Dot.CONNECTED, EndpointKind.TAILSCALE)
            settle()
            capture("02-chip-tailscale-$suffix.png")

            // Long-press: the quick switcher (a tap opens the computer page).
            compose.onNodeWithTag("connection-chip").performTouchInput { longClick() }
            settle()
            check(model.connectionSheet == ZeronModel.ConnectionSheet.SWITCHER)
            capture("03-switcher-$suffix.png")
            model.connectionSheet = null
            settle()

            // Connection details > 线路 (Routes), on Tailscale after LAN timed out.
            model.showLinkDetails = true
            settle()
            compose.onNodeWithText(app.getString(R.string.route_auto_hint)).assertExists()
            capture("04-details-$suffix.png")
            model.showLinkDetails = false
            settle()

            // Neither address answers: per-address reasons.
            model.network = outside.copy(vpn = NetworkSnapshot.Vpn.OTHER)
            model.directStatus = status(DirectPhase.FAILED, down, error = "no route to host (os error 113) reaching 100.124.7.39:22", retryAtMs = System.currentTimeMillis() + 12_500)
            pin(Dot.FAILED, null, error = "no route to host (os error 113) reaching 100.124.7.39:22", retryAtMs = System.currentTimeMillis() + 12_500)
            settle()
            // The failure sheet pops by itself once per episode (a chip tap opens the computer page).
            model.connectionSheet = ZeronModel.ConnectionSheet.FAILURE
            settle()
            check(model.connectionSheet == ZeronModel.ConnectionSheet.FAILURE)
            compose.onNodeWithTag("failure-endpoints", useUnmergedTree = true).assertExists()
            capture("05-failure-per-address-$suffix.png")
            model.connectionSheet = null
            settle()

            // Edit computer: the address list, then the merge picker.
            model.editMachine = model.machines.first { it.id == villa.id }
            settle()
            compose.onNodeWithTag("addresses", useUnmergedTree = true).assertExists()
            capture("06-edit-addresses-$suffix.png")
            compose.onNodeWithText(app.getString(R.string.merge_computer)).performScrollTo()
            settle()
            capture("07-edit-addresses-scrolled-$suffix.png")
            compose.onNodeWithText(app.getString(R.string.merge_computer)).performClick()
            settle()
            capture("08-merge-picker-$suffix.png")
            compose.onNodeWithText(app.getString(R.string.cancel)).performClick()
            model.editMachine = null
            settle()
        }
        model.previewConnection = null
        model.directStatus = null
        model.activeMachine = "demo"
        model.applyAppearance(2)
        scenario.close()
    }

    /** Settings > 自动选择线路 (Auto-select route), and with it off: picking the address by hand. */
    @Test
    fun manualRouteScreens() {
        val app = ApplicationProvider.getApplicationContext<Application>()
        org.robolectric.shadows.ShadowBuild.setModel("Pixel 8")
        FakeAndroidKeyStore.install()
        app.getSharedPreferences("zeron-update", 0).edit().putLong("lastCheck", System.currentTimeMillis()).putBoolean("autoUpdate", false).commit()
        MachineStore(app).apply {
            save(villa, null)
            save(laptop, null)
        }
        val scenario = ActivityScenario.launch(MainActivity::class.java)
        lateinit var model: ZeronModel
        scenario.onActivity { model = ViewModelProvider(it)[ZeronModel::class.java] }
        settleUntil("demo workspace") { model.phase == ZeronModel.Phase.Ready && model.workspace != null }
        model.applyListMode(ZeronModel.ListMode.Project)
        val away = listOf(
            stat(lan, ago = 97_000, error = "timed out reaching 192.168.1.102:22"),
            stat(tailscale, active = true, ago = 97_000, okAgo = 95_500, latency = 212),
        )
        val saved = model.machines.first { it.id == villa.id }

        for ((mode, suffix) in listOf(2 to "dark", 1 to "light")) {
            model.applyAppearance(mode)
            model.activeMachine = villa.id
            model.network = outside
            model.directStatus = status(DirectPhase.LIVE, away)
            model.previewConnection = ConnectionState.View("Villa", Workspace.DIRECT, Dot.CONNECTED, id = villa.id, route = EndpointKind.TAILSCALE)
            if (!model.autoRoute) model.applyAutoRoute(true)

            // Settings: the switch, on by default, then off with a tap.
            model.tab = ZeronModel.Tab.Settings
            settle()
            check(model.autoRoute)
            capture("09-settings-auto-route-on-$suffix.png")
            compose.onNodeWithTag("auto-route").performClick()
            settle()
            check(!model.autoRoute)
            check(!MachineStore(app).autoRoute) { "the switch persists" }
            // Turning it off keeps the link where it was: Tailscale is now the pick.
            check(model.pinnedAddress(saved) == tailscale)
            capture("10-settings-auto-route-off-$suffix.png")
            model.tab = ZeronModel.Tab.Sessions
            settle()

            // Switcher: Villa opens up into its addresses, the one in use ticked.
            // Long-press: the quick switcher (a tap opens the computer page).
            compose.onNodeWithTag("connection-chip").performTouchInput { longClick() }
            settle()
            compose.onNodeWithTag("switcher-routes-${villa.id}", useUnmergedTree = true).assertExists()
            capture("11-switcher-manual-route-$suffix.png")
            model.connectionSheet = null
            settle()

            // The picked address fails: the sheet offers the other route.
            model.directStatus = status(DirectPhase.FAILED, listOf(stat(tailscale, ago = 8_000, error = "no route to host (os error 113) reaching 100.124.7.39:22")), error = "no route to host (os error 113) reaching 100.124.7.39:22", retryAtMs = System.currentTimeMillis() + 12_500)
            model.previewConnection = ConnectionState.View("Villa", Workspace.DIRECT, Dot.FAILED, "no route to host (os error 113) reaching 100.124.7.39:22", System.currentTimeMillis() + 12_500, id = villa.id)
            model.connectionSheet = ZeronModel.ConnectionSheet.FAILURE
            settle()
            compose.onNodeWithTag("failure-routes", useUnmergedTree = true).assertExists()
            capture("12-failure-switch-route-$suffix.png")
            model.connectionSheet = null
            settle()

            // Edit computer: the addresses become a radio list.
            model.editMachine = saved
            settle()
            capture("13-edit-manual-route-$suffix.png")
            model.editMachine = null
            settle()
        }
        model.applyAutoRoute(true)
        model.previewConnection = null
        model.directStatus = null
        model.activeMachine = "demo"
        model.applyAppearance(2)
        scenario.close()
    }

    private fun capture(name: String) {
        compose.onRoot().captureRoboImage(Screenshots.path("device-groups/$name"))
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
