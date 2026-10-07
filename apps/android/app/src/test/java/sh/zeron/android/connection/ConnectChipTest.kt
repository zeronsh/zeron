package sh.zeron.android.connection

import android.app.Application
import android.content.ComponentName
import android.content.Intent
import android.content.pm.ActivityInfo
import android.content.pm.ApplicationInfo
import android.content.pm.PackageInfo
import android.os.Looper
import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.compose.ui.test.longClick
import androidx.compose.ui.test.onAllNodesWithTag
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollTo
import androidx.compose.ui.test.performTouchInput
import androidx.lifecycle.ViewModelProvider
import androidx.test.core.app.ActivityScenario
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config
import sh.zeron.android.MainActivity
import sh.zeron.android.R
import sh.zeron.android.core.ConnectionDiagnosis
import sh.zeron.android.core.ConnectionState
import sh.zeron.android.core.Endpoint
import sh.zeron.android.core.Machine
import sh.zeron.android.core.MachineStore
import sh.zeron.android.core.NetworkSnapshot
import sh.zeron.android.core.Tailscale
import sh.zeron.android.core.ZeronModel
import sh.zeron.android.screenshots.FakeAndroidKeyStore
import sh.zeron.android.screenshots.Screenshots
import uniffi.zeron_core.DirectEndpointStat
import uniffi.zeron_core.DirectPhase
import uniffi.zeron_core.DirectStatus

/**
 * Home chip: tap opens the computer's page (state, reason, fix), long-press
 * the quick switcher. Away from home with Tailscale off the chip turns red
 * before the dial gives up, and "Open Tailscale" launches its app.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34], qualifiers = "w411dp-h891dp-xxhdpi")
class ConnectChipTest {
    @get:Rule
    val compose = createEmptyComposeRule()

    @Before
    fun gate() = Screenshots.assumeHostCore()

    private val lan = Endpoint("192.168.1.102")
    private val ts = Endpoint("100.124.7.39")
    private val villa = Machine(id = "chip-villa", name = "Villa", user = "tx_vi", hostKey = "SHA256:v").withAddresses(listOf(lan, ts))
    private val cell = NetworkSnapshot(NetworkSnapshot.Transport.CELLULAR, vpn = NetworkSnapshot.Vpn.NONE)

    private fun launch(): Pair<ActivityScenario<MainActivity>, ZeronModel> {
        FakeAndroidKeyStore.install()
        val scenario = ActivityScenario.launch(MainActivity::class.java)
        lateinit var model: ZeronModel
        scenario.onActivity { model = ViewModelProvider(it)[ZeronModel::class.java] }
        val app = model.getApplication<Application>()
        app.getSharedPreferences("zeron-update", 0).edit().putLong("lastCheck", System.currentTimeMillis()).putBoolean("autoUpdate", false).commit()
        MachineStore(app).save(villa, null)
        model.saveMachine(villa, null)
        settleUntil("home") { model.phase == ZeronModel.Phase.Ready && model.workspace != null }
        if (model.activeMachine != "demo") {
            model.enterDemo()
            settleUntil("the demo") { model.phase == ZeronModel.Phase.Ready && model.activeMachine == "demo" }
        }
        model.tab = ZeronModel.Tab.Sessions
        return scenario to model
    }

    private fun reset(model: ZeronModel) {
        model.previewConnection = null
        model.connectionSheet = null
        model.editMachine = null
        model.showMachines = false
        model.tab = ZeronModel.Tab.Sessions
        settle()
    }

    private fun failingView(model: ZeronModel) = ConnectionState.View(
        "Villa", ConnectionState.Workspace.DIRECT, ConnectionState.Dot.FAILED, id = villa.id,
        diagnosis = ConnectionDiagnosis.Result(ConnectionDiagnosis.Kind.AWAY_TAILSCALE_OFF, sh.zeron.android.core.ConnectionIssue.Kind.UNREACHABLE, early = true),
    )

    @Test
    fun chipTapOpensTheComputerPageAndLongPressTheSwitcher() {
        val (scenario, model) = launch()
        val app = model.getApplication<Application>()
        model.previewConnection = failingView(model)
        settle()
        compose.onNodeWithTag("connection-chip").performClick()
        settle()
        // Settings > Accounts & Computers > Villa, with the reason and the fix on top.
        assertEquals(ZeronModel.Tab.Settings, model.tab)
        assertTrue(model.showMachines)
        assertEquals(villa.id, model.editMachine?.id)
        assertNull(model.connectionSheet)
        compose.onNodeWithTag("computer-status").assertExists()
        compose.onNodeWithText(app.getString(R.string.conn_diag_away_tailscale_off)).assertExists()
        compose.onNodeWithTag("tailscale-action").assertExists()
        // The chip is a shortcut path: one Back collapses all of it home.
        assertTrue(model.back())
        assertEquals(ZeronModel.Tab.Sessions, model.tab)
        assertNull(model.editMachine)
        assertTrue(!model.showMachines)

        // The slow path (Settings > computers > editor) still unwinds a
        // level at a time: Back lands on the computers list.
        model.tab = ZeronModel.Tab.Settings
        model.showMachines = true
        model.editMachine = villa
        settle()
        assertTrue(model.back())
        assertNull(model.editMachine)
        assertTrue(model.showMachines)

        // "Switch computer" on the page: back home, switcher open.
        model.editMachine = villa
        settle()
        compose.onNodeWithText(app.getString(R.string.switch_computer)).performScrollTo().performClick()
        settle()
        assertEquals(ZeronModel.Tab.Sessions, model.tab)
        assertNull(model.editMachine)
        assertEquals(ZeronModel.ConnectionSheet.SWITCHER, model.connectionSheet)
        model.connectionSheet = null
        settle()

        // Long-press: the quick switcher straight away.
        compose.onNodeWithTag("connection-chip").performTouchInput { longClick() }
        settle()
        assertEquals(ZeronModel.ConnectionSheet.SWITCHER, model.connectionSheet)
        compose.onNodeWithTag("connection-switcher").assertExists()
        reset(model)
        scenario.close()
    }

    @Test
    fun demoChipOpensTheComputersList() {
        val (scenario, model) = launch()
        compose.onNodeWithTag("connection-chip").performClick()
        settle()
        assertTrue(model.showMachines)
        assertNull(model.editMachine)
        // The chip is still a shortcut: one Back lands home, not on Settings.
        assertTrue(model.back())
        assertEquals(ZeronModel.Tab.Sessions, model.tab)
        assertTrue(!model.showMachines)
        reset(model)
        scenario.close()
    }

    @Test
    fun awayWithTailscaleOffTurnsRedBeforeTheDialGivesUp() {
        val (scenario, model) = launch()
        model.tailscaleInstalled = true
        model.network = cell
        model.activeMachine = villa.id
        model.directStatus = connecting()
        val view = model.connectionView()
        assertEquals(ConnectionState.Dot.FAILED, view.dot)
        assertEquals(ConnectionDiagnosis.Kind.AWAY_TAILSCALE_OFF, view.diagnosis?.kind)
        assertTrue(view.diagnosis!!.early)
        // Tailscale comes up: back to the core's own state (still dialling).
        model.network = cell.copy(vpn = NetworkSnapshot.Vpn.TAILSCALE)
        assertEquals(ConnectionState.Dot.CONNECTING, model.connectionView().dot)
        assertNull(model.connectionView().diagnosis)
        // Not installed: says so instead.
        model.network = cell
        model.tailscaleInstalled = false
        assertEquals(ConnectionDiagnosis.Kind.TAILSCALE_MISSING, model.connectionView().diagnosis?.kind)
        model.activeMachine = "demo"
        model.directStatus = null
        model.network = NetworkSnapshot.UNKNOWN
        reset(model)
        scenario.close()
    }

    @Test
    fun openTailscaleLaunchesItsAppOrItsStorePage() {
        val (scenario, model) = launch()
        val app = model.getApplication<Application>()
        val shadowApp = shadowOf(app)
        while (shadowApp.nextStartedActivity != null) Unit
        // Not installed: a note, then the store page.
        model.openTailscale()
        val store = shadowApp.nextStartedActivity
        assertNotNull(store)
        assertTrue(store.data.toString(), store.data.toString().contains(Tailscale.PACKAGE))
        // Installed: its launcher activity.
        installTailscale(app)
        assertTrue(Tailscale.installed(app))
        model.previewConnection = failingView(model)
        model.connectionSheet = ZeronModel.ConnectionSheet.FAILURE
        settle()
        compose.onNodeWithTag("tailscale-action").performClick()
        val launched = shadowApp.nextStartedActivity
        assertEquals(Tailscale.PACKAGE, launched?.component?.packageName ?: launched?.`package`)
        reset(model)
        scenario.close()
    }

    private fun installTailscale(app: Application) {
        val pm = shadowOf(app.packageManager)
        val info = PackageInfo().apply {
            packageName = Tailscale.PACKAGE
            applicationInfo = ApplicationInfo().apply { packageName = Tailscale.PACKAGE; name = "Tailscale" }
        }
        pm.installPackage(info)
        val component = ComponentName(Tailscale.PACKAGE, "com.tailscale.ipn.MainActivity")
        pm.addActivityIfNotPresent(component)
        val launcher = Intent(Intent.ACTION_MAIN).addCategory(Intent.CATEGORY_LAUNCHER).setPackage(Tailscale.PACKAGE)
        pm.addIntentFilterForActivity(component, android.content.IntentFilter(Intent.ACTION_MAIN).apply { addCategory(Intent.CATEGORY_LAUNCHER) })
        pm.addResolveInfoForIntent(launcher, android.content.pm.ResolveInfo().apply {
            activityInfo = ActivityInfo().apply { packageName = Tailscale.PACKAGE; name = component.className; applicationInfo = info.applicationInfo }
        })
    }

    private fun connecting(): DirectStatus = DirectStatus(
        phase = DirectPhase.CONNECTING, lastError = null, retryAtMs = null, engineVersion = null, engineDeviceId = null, notice = null,
        connectedAtMs = null, syncedAtMs = null, streams = emptyList(), log = emptyList(), clockOffsetMs = null,
        endpoints = listOf(lan, ts).map { DirectEndpointStat(it.host, it.port.toUShort(), it.kind.wire, false, null, null, null, null) },
    )

    private fun settle(ms: Long = 600) {
        val end = System.currentTimeMillis() + ms
        while (System.currentTimeMillis() < end) {
            shadowOf(Looper.getMainLooper()).idleFor(java.time.Duration.ofMillis(50))
            compose.mainClock.advanceTimeBy(50)
            Thread.sleep(20)
        }
        compose.waitForIdle()
    }

    private fun settleUntil(what: String, timeoutMs: Long = 30_000, done: () -> Boolean) {
        val end = System.currentTimeMillis() + timeoutMs
        while (!done()) {
            check(System.currentTimeMillis() < end) { "timed out waiting for $what" }
            settle(200)
        }
    }
}
