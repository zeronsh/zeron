package sh.zeron.android.connection

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import sh.zeron.android.core.ConnectionDiagnosis
import sh.zeron.android.core.ConnectionDiagnosis.Kind
import sh.zeron.android.core.ConnectionIssue
import sh.zeron.android.core.Endpoint
import sh.zeron.android.core.NetworkSnapshot
import sh.zeron.android.core.NetworkSnapshot.Transport
import sh.zeron.android.core.NetworkSnapshot.Vpn
import sh.zeron.android.core.Subnet

/** Why a computer can't be reached, from the phone's network: fail fast, then explain. */
class ConnectionDiagnosisTest {
    private val lan = Endpoint("192.168.1.102")
    private val ts = Endpoint("100.124.7.39")
    private val home = NetworkSnapshot(Transport.WIFI, listOf(Subnet.of("192.168.1.57", 24)!!), Vpn.NONE)
    private val homeTs = home.copy(vpn = Vpn.TAILSCALE)
    private val cell = NetworkSnapshot(Transport.CELLULAR, vpn = Vpn.NONE)
    private val cellTs = cell.copy(vpn = Vpn.TAILSCALE)
    private val cellClash = cell.copy(vpn = Vpn.OTHER)

    private fun auto(net: NetworkSnapshot, vararg addresses: Endpoint, installed: Boolean = true) =
        ConnectionDiagnosis.Input(addresses.toList(), addresses.toList(), net, installed, autoRoute = true)

    private fun manual(net: NetworkSnapshot, picked: Endpoint, vararg addresses: Endpoint) =
        ConnectionDiagnosis.Input(addresses.toList(), listOf(picked), net, tailscaleInstalled = true, autoRoute = false)

    @Test fun awayWithTailscaleOffFailsFastWithTheFix() {
        val r = ConnectionDiagnosis.preflight(auto(cell, lan, ts))!!
        assertEquals(Kind.AWAY_TAILSCALE_OFF, r.kind)
        assertTrue(r.early)
        assertTrue(r.opensTailscale)
        assertFalse(r.otherVpn)
    }

    @Test fun onlyTailscaleAndItIsOff() {
        val r = ConnectionDiagnosis.preflight(auto(cell, ts))!!
        assertEquals(Kind.TAILSCALE_OFF, r.kind)
        assertTrue(r.opensTailscale)
    }

    @Test fun anotherVpnHoldsTheSlot() {
        val r = ConnectionDiagnosis.preflight(auto(cellClash, lan, ts))!!
        assertEquals(Kind.AWAY_TAILSCALE_OFF, r.kind)
        assertTrue(r.otherVpn)
    }

    @Test fun tailscaleNotInstalledSaysSo() {
        val r = ConnectionDiagnosis.preflight(auto(cell, lan, ts, installed = false))!!
        assertEquals(Kind.TAILSCALE_MISSING, r.kind)
        assertTrue(r.installsTailscale)
        assertFalse(r.opensTailscale)
    }

    @Test fun nothingToSayWhenSomethingCanWork() {
        assertNull(ConnectionDiagnosis.preflight(auto(home, lan, ts)))
        assertNull(ConnectionDiagnosis.preflight(auto(cellTs, lan, ts)))
        // VPN up but its addresses unseen: maybe Tailscale, so let it dial.
        assertNull(ConnectionDiagnosis.preflight(auto(cell.copy(vpn = Vpn.UNKNOWN), lan, ts)))
        // A public address may work from anywhere.
        assertNull(ConnectionDiagnosis.preflight(auto(cell, lan, Endpoint("villa.example.com"))))
    }

    @Test fun phoneOffline() {
        val r = ConnectionDiagnosis.preflight(auto(NetworkSnapshot(Transport.NONE), lan, ts))!!
        assertEquals(Kind.NO_NETWORK, r.kind)
        assertTrue(r.early)
    }

    @Test fun autoSelectOffStillWarnsWhenThePickedRouteIsTailscale() {
        // At home, LAN would work, but the user picked Tailscale and it's off.
        val r = ConnectionDiagnosis.preflight(manual(home, ts, lan, ts))!!
        assertEquals(Kind.TAILSCALE_OFF, r.kind)
        assertTrue(r.opensTailscale)
        // Picked LAN out on mobile data, though a Tailscale address exists.
        val picked = ConnectionDiagnosis.preflight(manual(cellTs, lan, lan, ts))!!
        assertEquals(Kind.NOT_SAME_WIFI, picked.kind)
        assertTrue(picked.pickedLan)
        // Picked LAN at home: fine.
        assertNull(ConnectionDiagnosis.preflight(manual(home, lan, lan, ts)))
    }

    @Test fun lanOnlyComputerAwayFromHome() {
        val r = ConnectionDiagnosis.preflight(auto(cellTs, lan))!!
        assertEquals(Kind.NOT_SAME_WIFI, r.kind)
        assertFalse(r.pickedLan)
        assertFalse(r.opensTailscale)
    }

    @Test fun lanFailedAtHomeAndTailscaleOff() {
        val errors = mapOf(lan.key to "timed out reaching 192.168.1.102:22", ts.key to "timed out reaching 100.124.7.39:22")
        val r = ConnectionDiagnosis.diagnose(auto(home, lan, ts), "timed out reaching 192.168.1.102:22", errors)
        assertEquals(Kind.TAILSCALE_OFF, r.kind)
        assertTrue(r.lanFailed)
        assertFalse(r.early)
        assertTrue(r.opensTailscale)
    }

    @Test fun tailscaleOnButTheComputerIsSilent() {
        val r = ConnectionDiagnosis.diagnose(auto(cellTs, lan, ts), "timed out reaching 100.124.7.39:22", mapOf(ts.key to "timed out reaching 100.124.7.39:22"))
        assertEquals(Kind.PC_UNREACHABLE, r.kind)
        assertEquals(ConnectionIssue.Kind.TIMEOUT, r.issue)
        assertFalse(r.opensTailscale)
    }

    @Test fun lanOnlyAtHomeAndSilent() {
        val r = ConnectionDiagnosis.diagnose(auto(homeTs, lan), "no route to host (os error 113) reaching 192.168.1.102:22")
        assertEquals(Kind.LAN_UNREACHABLE, r.kind)
    }

    @Test fun signInAndEngineProblemsComeFirst() {
        val auth = ConnectionDiagnosis.diagnose(auto(cellTs, lan, ts), "the machine rejected this phone's key for user dev")
        assertEquals(Kind.ISSUE, auth.kind)
        assertEquals(ConnectionIssue.Kind.AUTH_KEY, auth.issue)
        val password = ConnectionDiagnosis.diagnose(auto(home, lan, ts), "wrong password for user dev")
        assertEquals(ConnectionIssue.Kind.AUTH_PASSWORD, password.issue)
        val engine = ConnectionDiagnosis.diagnose(auto(home, lan, ts), "the machine refused a tunnel to 127.0.0.1:27654 (x). Is Zeron running there?")
        assertEquals(Kind.ISSUE, engine.kind)
        assertEquals(ConnectionIssue.Kind.ENGINE, engine.issue)
    }

    @Test fun refusedMeansSshIsOff() {
        val r = ConnectionDiagnosis.diagnose(auto(homeTs, lan, ts), "192.168.1.102:22 refused the connection")
        assertEquals(Kind.ISSUE, r.kind)
        assertEquals(ConnectionIssue.Kind.REFUSED, r.issue)
    }

    @Test fun failureWhileTheNetworkStillExplainsIt() {
        // The dial gave up while away with Tailscale off: same reason, no longer "early".
        val r = ConnectionDiagnosis.diagnose(auto(cell, lan, ts), "timed out reaching 100.124.7.39:22")
        assertEquals(Kind.AWAY_TAILSCALE_OFF, r.kind)
        assertFalse(r.early)
        assertEquals(ConnectionIssue.Kind.TIMEOUT, r.issue)
    }
}
