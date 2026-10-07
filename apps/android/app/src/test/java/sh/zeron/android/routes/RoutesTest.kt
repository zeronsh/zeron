package sh.zeron.android.routes

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import sh.zeron.android.core.Endpoint
import sh.zeron.android.core.EndpointKind
import sh.zeron.android.core.Machine
import sh.zeron.android.core.MachineGroups
import sh.zeron.android.core.NetworkSnapshot
import sh.zeron.android.core.NetworkSnapshot.Transport
import sh.zeron.android.core.NetworkSnapshot.Vpn
import sh.zeron.android.core.RoutePlanner
import sh.zeron.android.core.RouteSwitch
import sh.zeron.android.core.Subnet

/** Address kinds, dial order per network, when to move, and grouping duplicates. */
class RoutesTest {
    private val lan = Endpoint("192.168.1.102")
    private val ts = Endpoint("100.124.7.39")
    private val home = NetworkSnapshot(Transport.WIFI, listOf(Subnet.of("192.168.1.57", 24)!!), Vpn.TAILSCALE)
    private val homeNoVpn = home.copy(vpn = Vpn.NONE)
    private val cafe = NetworkSnapshot(Transport.WIFI, listOf(Subnet.of("10.20.30.40", 22)!!), Vpn.TAILSCALE)
    private val cellTs = NetworkSnapshot(Transport.CELLULAR, vpn = Vpn.TAILSCALE)
    private val cellClash = NetworkSnapshot(Transport.CELLULAR, vpn = Vpn.OTHER)

    @Test fun classifiesAddresses() {
        for (h in listOf("192.168.1.102", "10.0.0.5", "172.16.3.4", "172.31.255.1", "169.254.1.1", "villa.local", "nas.lan", "pc.home.arpa", "fe80::1", "[fd12:3456::1]")) {
            assertEquals(h, EndpointKind.LAN, EndpointKind.of(h))
        }
        for (h in listOf("100.124.7.39", "100.64.0.1", "100.127.255.254", "villa.tail1234.ts.net", "villa.tail1234.ts.net.", "fd7a:115c:a1e0::1")) {
            assertEquals(h, EndpointKind.TAILSCALE, EndpointKind.of(h))
        }
        // Just outside the ranges, public addresses and plain names.
        for (h in listOf("100.63.255.255", "100.128.0.1", "172.32.0.1", "8.8.8.8", "example.com", "villa", "2001:db8::1", "127.0.0.1")) {
            assertEquals(h, EndpointKind.OTHER, EndpointKind.of(h))
        }
    }

    @Test fun parsesTypedAddresses() {
        assertEquals(Endpoint("192.168.1.102", 22), Endpoint.parse(" 192.168.1.102 "))
        assertEquals(Endpoint("100.124.7.39", 2222), Endpoint.parse("100.124.7.39:2222"))
        assertEquals(Endpoint("fd7a:115c:a1e0::1", 22), Endpoint.parse("fd7a:115c:a1e0::1"))
        assertEquals(Endpoint("fd7a::1", 2200), Endpoint.parse("[fd7a::1]:2200"))
        assertNull(Endpoint.parse("host:notaport"))
        assertNull(Endpoint.parse("  "))
        assertEquals(Endpoint("Villa.LOCAL").key, Endpoint("villa.local").key)
    }

    @Test fun subnetMath() {
        val s = Subnet.of("192.168.1.57", 24)!!
        assertEquals("192.168.1.0/24", s.toString())
        assertTrue(s.contains("192.168.1.102"))
        assertTrue(!s.contains("192.168.2.102"))
        assertTrue(Subnet.of("10.20.30.40", 22)!!.contains("10.20.28.1"))
    }

    private fun order(net: NetworkSnapshot, vararg list: Endpoint, remembered: String? = null) =
        RoutePlanner.plan(list.toList(), net, remembered).map { it.endpoint }

    @Test fun homeWifiTriesTheLanFirstWithAShortHeadStart() {
        val plan = RoutePlanner.plan(listOf(ts, lan), home)
        assertEquals(listOf(lan, ts), plan.map { it.endpoint })
        assertEquals(listOf(RoutePlanner.LAN_HEAD_START_MS, 0), plan.map { it.headStartMs })
        // Tailscale off at home: the LAN still first, Tailscale last.
        assertEquals(listOf(lan, ts), order(homeNoVpn, ts, lan))
    }

    @Test fun awayFromHomeTailscaleGoesFirst() {
        assertEquals(listOf(ts, lan), order(cafe, lan, ts))
        assertEquals(listOf(ts, lan), order(cellTs, lan, ts))
        val plan = RoutePlanner.plan(listOf(lan, ts), cellTs)
        assertEquals(RoutePlanner.HEAD_START_MS, plan.first().headStartMs)
        assertTrue(plan.last().tier >= RoutePlanner.UNREACHABLE) // LAN kept, but last
    }

    @Test fun withClashHoldingTheVpnSlotAPublicAddressBeatsTailscale() {
        val public = Endpoint("villa.example.com")
        // Tailscale can't be up (Clash holds the VPN slot); LAN is off Wi-Fi.
        assertEquals(listOf(public, ts, lan), order(cellClash, ts, lan, public))
    }

    @Test fun manualRouteIsOnlyThePickedAddress() {
        val one = RoutePlanner.manual(listOf(lan, ts), ts.key)
        assertEquals(listOf(ts), one.map { it.endpoint })
        assertEquals(0, one.single().headStartMs)
        // Nothing picked yet, or the pick was removed: the first address alone.
        assertEquals(listOf(lan), RoutePlanner.manual(listOf(lan, ts), null).map { it.endpoint })
        assertEquals(listOf(lan), RoutePlanner.manual(listOf(lan, ts), "10.0.0.9:22").map { it.endpoint })
        assertEquals(emptyList<RoutePlanner.Planned>(), RoutePlanner.manual(emptyList(), ts.key))
    }

    @Test fun whatWorkedOnThisNetworkLastTimeGoesFirst() {
        val other = Endpoint("villa.example.com")
        assertEquals(listOf(other, ts, lan), order(cafe, lan, ts, other, remembered = other.key))
        // ...unless it can't work here any more (remembered LAN, now on mobile data).
        assertEquals(listOf(ts, lan), order(cellTs, lan, ts, remembered = lan.key))
    }

    @Test fun keepsTheUsersOrderWithinATierAndDropsDuplicates() {
        val a = Endpoint("a.example.com")
        val b = Endpoint("b.example.com")
        assertEquals(listOf(b, a), order(cellTs, b, a, Endpoint("B.example.com")).filter { it.kind == EndpointKind.OTHER })
        assertEquals(1, RoutePlanner.plan(listOf(lan), home).size)
        assertEquals(0, RoutePlanner.plan(listOf(lan), home).single().headStartMs)
    }

    @Test fun networkKeyUsesTheWifiSubnetNotItsName() {
        assertEquals("wifi:192.168.1.0/24", home.key)
        assertEquals("cellular", cellTs.key)
    }

    // ── moving the link on a network change ──

    private val move = RouteSwitch.MIN_UPGRADE_GAP_MS

    @Test fun leavingHomeMovesOffTheLanAtOnce() {
        val plan = RoutePlanner.plan(listOf(lan, ts), cellTs)
        assertEquals(RouteSwitch.Action.RECONNECT, RouteSwitch.decide(plan, lan.key, RouteSwitch.Link.LIVE, sinceLastMoveMs = 0))
    }

    @Test fun comingHomeUpgradesToTheLanButNotTooOften() {
        val plan = RoutePlanner.plan(listOf(lan, ts), home)
        assertEquals(RouteSwitch.Action.RECONNECT, RouteSwitch.decide(plan, ts.key, RouteSwitch.Link.LIVE, sinceLastMoveMs = move))
        assertEquals(RouteSwitch.Action.STAY, RouteSwitch.decide(plan, ts.key, RouteSwitch.Link.LIVE, sinceLastMoveMs = move - 1))
    }

    @Test fun aLinkOnTheBestAddressStays() {
        assertEquals(RouteSwitch.Action.STAY, RouteSwitch.decide(RoutePlanner.plan(listOf(lan, ts), home), lan.key, RouteSwitch.Link.LIVE, 0))
        assertEquals(RouteSwitch.Action.STAY, RouteSwitch.decide(RoutePlanner.plan(listOf(lan, ts), cellTs), ts.key, RouteSwitch.Link.LIVE, 0))
    }

    @Test fun tailscaleSwitchedOffMovesAtOnce() {
        val plan = RoutePlanner.plan(listOf(lan, ts), homeNoVpn)
        assertEquals(RouteSwitch.Action.RECONNECT, RouteSwitch.decide(plan, ts.key, RouteSwitch.Link.LIVE, 0))
    }

    @Test fun aFailedOrDiallingLinkRedialsWithTheNewOrder() {
        val plan = RoutePlanner.plan(listOf(lan, ts), home)
        assertEquals(RouteSwitch.Action.RECONNECT, RouteSwitch.decide(plan, null, RouteSwitch.Link.FAILED, 0))
        assertEquals(RouteSwitch.Action.RECONNECT, RouteSwitch.decide(plan, null, RouteSwitch.Link.CONNECTING, 0))
        assertEquals(RouteSwitch.Action.STAY, RouteSwitch.decide(emptyList(), null, RouteSwitch.Link.FAILED, 0))
    }

    // ── grouping saved computers ──

    private fun machine(id: String, name: String, host: String, key: String? = "SHA256:villa", user: String = "tx_vi") =
        Machine(id = id, name = name, host = host, user = user, hostKey = key)

    @Test fun mergesTheSameComputerSavedUnderTwoAddresses() {
        val result = MachineGroups.merge(listOf(machine("a", "Villa", "192.168.1.102"), machine("b", "Villa (Tailscale)", "100.124.7.39")))
        assertEquals(1, result.machines.size)
        val villa = result.machines.single()
        assertEquals("a", villa.id)
        assertEquals("Villa", villa.name)
        assertEquals(listOf(lan, ts), villa.addresses())
        assertEquals("192.168.1.102", villa.host)
        assertEquals(mapOf("b" to "a"), result.mergedInto)
    }

    @Test fun mergesByNameWhenAHostKeyIsMissing() {
        val result = MachineGroups.merge(listOf(machine("a", "Villa", "192.168.1.102", key = null), machine("b", "villa ", "100.124.7.39")))
        assertEquals(listOf(lan, ts), result.machines.single().addresses())
        assertEquals("SHA256:villa", result.machines.single().hostKey)
    }

    @Test fun keepsDifferentComputersApart() {
        val keys = MachineGroups.merge(listOf(machine("a", "Villa", "192.168.1.102"), machine("b", "Villa", "100.124.7.39", key = "SHA256:other")))
        assertEquals(2, keys.machines.size) // same name, different host keys
        val users = MachineGroups.merge(listOf(machine("a", "Villa", "192.168.1.102"), machine("b", "Villa", "100.124.7.39", user = "admin")))
        assertEquals(2, users.machines.size)
        val auth = MachineGroups.merge(listOf(machine("a", "Villa", "192.168.1.102"), machine("b", "Villa", "100.124.7.39").copy(auth = Machine.AUTH_PASSWORD)))
        assertEquals(2, auth.machines.size)
        val unnamed = MachineGroups.merge(listOf(machine("a", "", "192.168.1.102", key = null), machine("b", "", "100.124.7.39", key = null)))
        assertEquals(2, unnamed.machines.size)
        assertTrue(unnamed.mergedInto.isEmpty())
    }

    @Test fun mergingTheSameAddressTwiceDoesNotDuplicateIt() {
        val result = MachineGroups.merge(listOf(machine("a", "Villa", "192.168.1.102"), machine("b", "Villa", "192.168.1.102"), machine("c", "Villa", "100.124.7.39")))
        assertEquals(listOf(lan, ts), result.machines.single().addresses())
        assertEquals(mapOf("b" to "a", "c" to "a"), result.mergedInto)
    }
}
