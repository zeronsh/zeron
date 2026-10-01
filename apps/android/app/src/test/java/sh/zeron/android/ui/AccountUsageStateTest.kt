package sh.zeron.android.ui

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.runBlocking
import org.junit.Assert.*
import org.junit.Test
import sh.zeron.android.core.Agents

class AccountUsageStateTest {
    private val snapshot = Agents.Accounts(emptyList(), emptyMap())

    @Test fun cachedProbePaintsBeforeForcedRefreshAndRepeatedHoldsAreThrottled() = runBlocking {
        var now = 0L
        val requests = mutableListOf<Boolean>()
        val state = AccountUsageState(clock = { now }) { force -> requests += force; snapshot }
        state.refresh(force = false)
        assertSame(snapshot, state.snapshot)
        state.refresh(force = true)
        now = 29_999
        state.refresh(force = true)
        assertEquals(listOf(false, true), requests)
        now = 30_000
        state.refresh(force = true)
        assertEquals(listOf(false, true, true), requests)
        assertFalse(state.loading)
    }

    @Test fun failedRefreshKeepsTheLastSnapshotAndRecoveryClearsTheError() = runBlocking {
        var fail = false
        val state = AccountUsageState {
            if (fail) throw IllegalStateException("Host offline")
            snapshot
        }
        state.refresh(force = false)
        fail = true
        state.refresh(force = false)
        assertSame(snapshot, state.snapshot)
        assertNotNull(state.error)
        assertFalse(state.loading)
        fail = false
        state.refresh(force = false)
        assertNull(state.error)
    }

    @Test fun lifecycleCancellationReleasesTheRefreshAndAllowsImmediateRetry() = runBlocking {
        var cancel = true
        var requests = 0
        val state = AccountUsageState(clock = { 0L }) {
            requests++
            if (cancel) throw CancellationException("App backgrounded")
            snapshot
        }
        try {
            state.refresh(force = true)
            fail("Cancellation must propagate")
        } catch (_: CancellationException) { }
        assertFalse(state.loading)
        assertNull(state.error)
        cancel = false
        state.refresh(force = true)
        assertSame(snapshot, state.snapshot)
        assertEquals(2, requests)
    }
}
