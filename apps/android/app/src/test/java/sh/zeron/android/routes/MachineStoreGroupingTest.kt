package sh.zeron.android.routes

import android.app.Application
import androidx.test.core.app.ApplicationProvider
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import sh.zeron.android.core.Endpoint
import sh.zeron.android.core.Machine
import sh.zeron.android.core.MachineStore
import sh.zeron.android.core.NetworkSnapshot
import sh.zeron.android.core.RoutePlanner
import sh.zeron.android.core.Subnet
import sh.zeron.android.screenshots.FakeAndroidKeyStore

/** Saved addresses, the one-time grouping migration, merge / split, and the core target. */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34])
class MachineStoreGroupingTest {
    private lateinit var store: MachineStore
    private val lan = Endpoint("192.168.1.102")
    private val ts = Endpoint("100.124.7.39")

    @Before fun setUp() {
        FakeAndroidKeyStore.install()
        store = MachineStore(ApplicationProvider.getApplicationContext<Application>())
    }

    @Test fun addressesSurviveASaveAndOldEntriesReadAsOneAddress() {
        store.save(Machine(id = "v", name = "Villa", user = "tx_vi").withAddresses(listOf(lan, ts)), null)
        store.save(Machine(id = "old", name = "Old", host = "10.0.0.9", port = 2222, user = "me"), null)
        val (villa, old) = store.list().let { l -> l.first { it.id == "v" } to l.first { it.id == "old" } }
        assertEquals(listOf(lan, ts), villa.addresses())
        assertEquals("192.168.1.102", villa.host)
        assertEquals(listOf(Endpoint("10.0.0.9", 2222)), old.addresses())
    }

    @Test fun groupingRunsOnceAndCarriesThePassword() {
        store.save(Machine(id = "a", name = "Villa", host = lan.host, user = "tx_vi", auth = Machine.AUTH_PASSWORD, hostKey = "SHA256:v"), null)
        store.save(Machine(id = "b", name = "Villa TS", host = ts.host, user = "tx_vi", auth = Machine.AUTH_PASSWORD, hostKey = "SHA256:v"), "hunter2")
        assertEquals(mapOf("b" to "a"), store.groupDuplicates())
        val villa = store.list().single()
        assertEquals(listOf(lan, ts), villa.addresses())
        assertEquals("hunter2", store.secret("a"))
        assertNull(store.secret("b"))
        // Split them again by hand: the migration must not re-merge them.
        store.split("a", ts, "Villa TS")
        assertEquals(emptyMap<String, String>(), store.groupDuplicates())
        assertEquals(2, store.list().size)
    }

    @Test fun manualMergeAndSplit() {
        store.save(Machine(id = "a", name = "Villa", host = lan.host, user = "tx_vi"), null)
        store.save(Machine(id = "b", name = "Laptop", host = ts.host, user = "tx_vi"), null)
        val merged = store.merge("a", "b")!!
        assertEquals(listOf(lan, ts), merged.addresses())
        assertEquals(listOf("a"), store.list().map { it.id })
        val alone = store.split("a", lan, "Villa LAN")!!
        assertEquals(listOf(lan), alone.addresses())
        assertEquals(listOf(ts), store.list().first { it.id == "a" }.addresses())
        assertEquals("100.124.7.39", store.list().first { it.id == "a" }.host)
        // The last address can't be split off.
        assertNull(store.split("a", ts, "x"))
    }

    @Test fun targetCarriesThePlannedOrder() {
        val villa = Machine(id = "v", name = "Villa", user = "tx_vi", hostKey = "SHA256:v").withAddresses(listOf(ts, lan))
        val home = NetworkSnapshot(NetworkSnapshot.Transport.WIFI, listOf(Subnet.of("192.168.1.5", 24)!!), NetworkSnapshot.Vpn.TAILSCALE)
        val target = store.target(villa, route = RoutePlanner.plan(villa.addresses(), home))
        assertEquals("192.168.1.102", target.host)
        assertEquals(listOf("lan", "tailscale"), target.endpoints.map { it.kind })
        assertEquals(listOf(1_500u, 0u), target.endpoints.map { it.headStartMs })
        // A silent LAN IP gives up in seconds; Tailscale gets longer for a DERP relay.
        assertEquals(listOf(4_000u, 12_000u), target.endpoints.map { it.connectTimeoutMs })
        assertTrue(target.endpoints.all { it.port.toInt() == 22 })
        // Default: saved order, one after another.
        assertEquals(listOf("100.124.7.39", "192.168.1.102"), store.target(villa).endpoints.map { it.host })
    }

    @Test fun autoSelectOffUsesThePickedAddressOnEveryNetwork() {
        val villa = Machine(id = "v", name = "Villa", user = "tx_vi", hostKey = "SHA256:v").withAddresses(listOf(lan, ts))
        val home = NetworkSnapshot(NetworkSnapshot.Transport.WIFI, listOf(Subnet.of("192.168.1.5", 24)!!), NetworkSnapshot.Vpn.TAILSCALE)
        val cell = NetworkSnapshot(NetworkSnapshot.Transport.CELLULAR, vpn = NetworkSnapshot.Vpn.TAILSCALE)
        assertTrue("auto-select is on by default", store.autoRoute)
        assertEquals(listOf(ts, lan), store.plan(villa, cell).map { it.endpoint })
        store.autoRoute = false
        // No pick yet: the first address, and only it (no fallback).
        assertEquals(listOf(lan), store.plan(villa, cell).map { it.endpoint })
        store.pinAddress(villa.id, ts.key)
        assertEquals(listOf(ts), store.plan(villa, home).map { it.endpoint })
        assertEquals(listOf(ts), store.plan(villa, cell).map { it.endpoint })
        // Both the switch and the pick survive a restart.
        val again = MachineStore(ApplicationProvider.getApplicationContext<Application>())
        assertEquals(false, again.autoRoute)
        assertEquals(ts, again.pinnedAddress(villa))
        again.autoRoute = true
        assertEquals(listOf(lan, ts), again.plan(villa, home).map { it.endpoint })
    }
}
