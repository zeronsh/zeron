package sh.zeron.android.connection

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import sh.zeron.android.core.ConnectionState
import sh.zeron.android.core.ConnectionState.Dot
import sh.zeron.android.core.ConnectionState.Workspace
import uniffi.zeron_core.ConnectivityState
import uniffi.zeron_core.DirectPhase

class ConnectionStateTest {
    @Test fun directPhases() {
        fun d(p: DirectPhase?) = ConnectionState.dot(Workspace.DIRECT, loading = false, direct = p, cloud = null)
        assertEquals(Dot.CONNECTING, d(null))
        assertEquals(Dot.CONNECTING, d(DirectPhase.CONNECTING))
        assertEquals(Dot.CONNECTING, d(DirectPhase.SYNCING))
        assertEquals(Dot.CONNECTED, d(DirectPhase.LIVE))
        assertEquals(Dot.FAILED, d(DirectPhase.FAILED))
        assertEquals(Dot.CONNECTING, ConnectionState.dot(Workspace.DIRECT, loading = true, direct = DirectPhase.LIVE, cloud = null))
    }

    @Test fun cloudAndDemo() {
        fun c(s: ConnectivityState?) = ConnectionState.dot(Workspace.CLOUD, loading = false, direct = null, cloud = s)
        assertEquals(Dot.CONNECTED, c(ConnectivityState.CONNECTED))
        assertEquals(Dot.FAILED, c(ConnectivityState.OFFLINE))
        assertEquals(Dot.CONNECTING, c(ConnectivityState.RECONNECTING))
        assertEquals(Dot.CONNECTING, c(null))
        assertEquals(Dot.NEUTRAL, ConnectionState.dot(Workspace.DEMO, loading = true, direct = null, cloud = null))
    }

    @Test fun failureSheetPopsOncePerEpisode() {
        val e = ConnectionState.Episodes()
        assertFalse(e.update(Dot.CONNECTING))
        assertTrue(e.update(Dot.FAILED))
        // Automatic retries: connecting, failing again: no second pop.
        assertFalse(e.update(Dot.CONNECTING))
        assertFalse(e.update(Dot.FAILED))
        assertFalse(e.update(Dot.FAILED))
        // Connected ends the episode; the next failure pops again.
        assertFalse(e.update(Dot.CONNECTED))
        assertTrue(e.update(Dot.FAILED))
        // Switching workspace starts over.
        e.reset()
        assertTrue(e.update(Dot.FAILED))
    }
}
