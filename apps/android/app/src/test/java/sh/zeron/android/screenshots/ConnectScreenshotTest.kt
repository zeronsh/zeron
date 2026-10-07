package sh.zeron.android.screenshots

import android.app.Application
import android.os.Looper
import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import androidx.lifecycle.ViewModelProvider
import androidx.test.core.app.ActivityScenario
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
import sh.zeron.android.core.ConnectionDiagnosis
import sh.zeron.android.core.ConnectionState
import sh.zeron.android.core.Endpoint
import sh.zeron.android.core.Machine
import sh.zeron.android.core.NetworkSnapshot
import sh.zeron.android.core.NetworkSnapshot.Transport
import sh.zeron.android.core.NetworkSnapshot.Vpn
import sh.zeron.android.core.Subnet
import sh.zeron.android.core.ZeronModel
import uniffi.zeron_core.DirectEndpointStat
import uniffi.zeron_core.DirectLogLine
import uniffi.zeron_core.DirectPhase
import uniffi.zeron_core.DirectStatus

/**
 * Connection diagnosis: the failure sheet for Tailscale off (on the home
 * Wi-Fi after LAN failed, and away from home before the dial gives up),
 * Tailscale not installed, another VPN, the computer asleep, LAN-only away
 * from home, sign-in and engine failures; then the computer page the chip
 * opens (down and connected) and Connection Details. Driven through the
 * real model state (active computer, network, link status), not a pinned
 * chip. Output: connect/ or zh/connect/.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [34], qualifiers = "w411dp-h891dp-night-xxhdpi")
open class ConnectScreenshotTest {
    protected open val subdir: String = ""

    @get:Rule
    val compose = createEmptyComposeRule()

    @Before
    fun gate() {
        Screenshots.assumeEnabled()
        Screenshots.assumeHostCore()
    }

    private val lan = Endpoint("192.168.1.102")
    private val ts = Endpoint("100.124.7.39")
    private val villa = Machine(id = "connect-villa", name = "Villa", user = "tx_vi", hostKey = "SHA256:villa").withAddresses(listOf(lan, ts))
    private val nas = Machine(id = "connect-nas", name = "NAS", host = "192.168.1.20", user = "admin", hostKey = "SHA256:nas")
    private val home = NetworkSnapshot(Transport.WIFI, listOf(Subnet.of("192.168.1.57", 24)!!), Vpn.NONE)
    private val cell = NetworkSnapshot(Transport.CELLULAR, vpn = Vpn.NONE)

    private fun stat(e: Endpoint, error: String? = null, active: Boolean = false, latency: Long? = null): DirectEndpointStat {
        val now = System.currentTimeMillis()
        return DirectEndpointStat(e.host, e.port.toUShort(), e.kind.wire, active, now - 6_000, if (active) now - 6_000 else null, error, latency?.toULong())
    }

    private fun status(phase: DirectPhase, endpoints: List<DirectEndpointStat>, error: String? = null): DirectStatus {
        val now = System.currentTimeMillis()
        val live = phase == DirectPhase.LIVE
        return DirectStatus(
            phase = phase, lastError = error, retryAtMs = if (phase == DirectPhase.FAILED) now + 12_500 else null,
            engineVersion = if (live) "0.9.4" else null, engineDeviceId = if (live) "a41c07f2e9b3" else null, notice = null,
            connectedAtMs = if (live) now - 95_000 else null, syncedAtMs = if (live) now - 2_000 else null,
            streams = emptyList(),
            log = endpoints.map { DirectLogLine(now - 6_000, "connecting to ${it.kind} ${it.host}:${it.port}") },
            clockOffsetMs = null, endpoints = endpoints,
        )
    }

    @OptIn(ExperimentalRoborazziApi::class)
    @Test
    fun connectDiagnosis() {
        org.robolectric.shadows.ShadowBuild.setModel("Pixel 8")
        FakeAndroidKeyStore.install()
        val scenario = ActivityScenario.launch(MainActivity::class.java)
        lateinit var model: ZeronModel
        scenario.onActivity { model = ViewModelProvider(it)[ZeronModel::class.java] }
        val app = model.getApplication<Application>()
        app.getSharedPreferences("zeron-update", 0).edit().putLong("lastCheck", System.currentTimeMillis()).putBoolean("autoUpdate", false).commit()
        settleUntil("demo workspace") { model.phase == ZeronModel.Phase.Ready && model.workspace != null }
        if (model.activeMachine != "demo") {
            model.enterDemo()
            settleUntil("the demo") { model.phase == ZeronModel.Phase.Ready && model.activeMachine == "demo" }
        }
        model.saveMachine(villa, null)
        model.saveMachine(nas, null)
        model.applyListMode(ZeronModel.ListMode.Project)
        model.previewConnection = null
        fun shot(name: String) = captureScreenRoboImage(Screenshots.path("${subdir}connect/$name"))

        /** Make [machine] the active one on [net] with [link]; open the failure sheet if it's red. */
        fun state(machine: Machine, net: NetworkSnapshot, link: DirectStatus, installed: Boolean = true, sheet: Boolean = true) {
            model.connectionSheet = null
            model.tab = ZeronModel.Tab.Sessions
            model.editMachine = null
            model.showMachines = false
            model.showLinkDetails = false
            model.activeMachine = machine.id
            model.network = net
            model.tailscaleInstalled = installed
            model.directStatus = link
            if (sheet && model.connectionView().dot == ConnectionState.Dot.FAILED) model.connectionSheet = ZeronModel.ConnectionSheet.FAILURE
            settle(900)
        }
        val tsTimeout = "timed out reaching 100.124.7.39:22"
        val lanTimeout = "timed out reaching 192.168.1.102:22"
        val connecting = status(DirectPhase.CONNECTING, listOf(stat(lan), stat(ts)))

        for ((mode, suffix) in listOf(2 to "dark", 1 to "light")) {
            model.applyAppearance(mode)
            settle(1000)

            // Home Wi-Fi, LAN didn't answer, Tailscale off on the phone: 「Tailscale 未启动」 ("Tailscale isn't running").
            state(villa, home, status(DirectPhase.FAILED, listOf(stat(lan, lanTimeout), stat(ts, tsTimeout)), lanTimeout))
            check(model.connectionView().diagnosis?.kind == ConnectionDiagnosis.Kind.TAILSCALE_OFF)
            compose.onNodeWithTag("tailscale-action").assertExists()
            shot("01-tailscale-off-$suffix.png")

            // Out on mobile data, Tailscale off: red at once, before any timeout.
            state(villa, cell, connecting)
            check(model.connectionView().diagnosis?.early == true)
            shot("02-away-tailscale-off-early-$suffix.png")

            // Same, Tailscale not installed.
            state(villa, cell, connecting, installed = false)
            check(model.connectionView().diagnosis?.kind == ConnectionDiagnosis.Kind.TAILSCALE_MISSING)
            shot("03-tailscale-not-installed-$suffix.png")

            // Another VPN holds the phone's one VPN slot.
            state(villa, cell.copy(vpn = Vpn.OTHER), connecting)
            shot("04-other-vpn-$suffix.png")

            // Tailscale on, the computer silent: asleep, or its Tailscale is off.
            state(villa, cell.copy(vpn = Vpn.TAILSCALE), status(DirectPhase.FAILED, listOf(stat(lan, "network is unreachable reaching 192.168.1.102:22"), stat(ts, tsTimeout)), tsTimeout))
            check(model.connectionView().diagnosis?.kind == ConnectionDiagnosis.Kind.PC_UNREACHABLE)
            shot("05-pc-unreachable-$suffix.png")

            // LAN-only computer: on its Wi-Fi but silent, then away from it.
            state(nas, home, status(DirectPhase.FAILED, listOf(stat(Endpoint("192.168.1.20"), "no route to host (os error 113) reaching 192.168.1.20:22")), "no route to host (os error 113) reaching 192.168.1.20:22"))
            check(model.connectionView().diagnosis?.kind == ConnectionDiagnosis.Kind.LAN_UNREACHABLE)
            shot("06-lan-unreachable-$suffix.png")
            state(nas, cell.copy(vpn = Vpn.TAILSCALE), status(DirectPhase.CONNECTING, listOf(stat(Endpoint("192.168.1.20")))))
            check(model.connectionView().diagnosis?.kind == ConnectionDiagnosis.Kind.NOT_SAME_WIFI)
            shot("07-not-same-wifi-$suffix.png")

            // SSH got through: sign-in refused, then Zeron not running.
            state(villa, home.copy(vpn = Vpn.TAILSCALE), status(DirectPhase.FAILED, listOf(stat(lan, "the machine rejected this phone's key for user tx_vi"), stat(ts)), "the machine rejected this phone's key for user tx_vi (add the phone's public key to authorized_keys)"))
            shot("08-ssh-auth-failed-$suffix.png")
            state(villa, home.copy(vpn = Vpn.TAILSCALE), status(DirectPhase.FAILED, listOf(stat(lan, "the machine refused a tunnel to 127.0.0.1:27654"), stat(ts)), "the machine refused a tunnel to 127.0.0.1:27654 (Connection refused). Is Zeron running there?"))
            shot("09-engine-not-running-$suffix.png")

            // Chip tap: Settings > Accounts & Computers > Villa, reason and fix on top.
            state(villa, cell, connecting, sheet = false)
            compose.onNodeWithTag("connection-chip").performClick()
            settle(900)
            check(model.editMachine?.id == villa.id && model.tab == ZeronModel.Tab.Settings)
            compose.onNodeWithTag("computer-status").assertExists()
            shot("10-computer-page-from-chip-down-$suffix.png")

            // Connected over LAN: the page's status line.
            state(villa, home.copy(vpn = Vpn.TAILSCALE), status(DirectPhase.LIVE, listOf(stat(lan, active = true, latency = 9), stat(ts))), sheet = false)
            compose.onNodeWithTag("connection-chip").performClick()
            settle(900)
            shot("11-computer-page-from-chip-connected-$suffix.png")

            // Connection Details: the reason above the link's state.
            state(villa, home, status(DirectPhase.FAILED, listOf(stat(lan, lanTimeout), stat(ts, tsTimeout)), lanTimeout), sheet = false)
            model.showLinkDetails = true
            settle(900)
            compose.onNodeWithTag("details-diagnosis").assertExists()
            shot("12-details-tailscale-off-$suffix.png")
        }
        model.showLinkDetails = false
        model.editMachine = null
        model.showMachines = false
        model.connectionSheet = null
        model.tab = ZeronModel.Tab.Sessions
        model.activeMachine = "demo"
        model.directStatus = null
        model.network = NetworkSnapshot.UNKNOWN
        model.applyAppearance(2)
        settle()
        scenario.close()
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
