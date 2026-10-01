package sh.zeron.android.core

import org.junit.Assert.*
import org.junit.Test

class LiveSubagentsTest {
    @Test fun anOpenChatsCountShowsAtOnceAndFollowsTheChat() {
        val live = LiveSubagents()
        assertTrue(live.report("a", 2, 0))
        assertEquals(mapOf("a" to 2), live.snapshot())
        assertTrue(live.report("a", 3, 10))
        assertFalse(live.report("a", 3, 20))
        assertNull(live.nextExpiry())
    }

    @Test fun aZeroReadIsHeldBrieflyThenDropped() {
        val live = LiveSubagents()
        live.report("a", 2, 0)
        assertFalse("a transient empty read does not flicker the badge", live.report("a", 0, 1_000))
        assertEquals(2, live.snapshot()["a"])
        assertEquals(1_000 + LiveSubagents.ZERO_GRACE_MS, live.nextExpiry())
        assertFalse(live.expire(1_000 + LiveSubagents.ZERO_GRACE_MS - 1))
        assertTrue(live.expire(1_000 + LiveSubagents.ZERO_GRACE_MS))
        assertTrue(live.snapshot().isEmpty())
    }

    @Test fun aSubagentThatComesBackCancelsThePendingDrop() {
        val live = LiveSubagents()
        live.report("a", 1, 0)
        live.report("a", 0, 100)
        live.report("a", 1, 200)
        assertNull(live.nextExpiry())
        assertFalse(live.expire(10_000))
        assertEquals(1, live.snapshot()["a"])
    }

    @Test fun aClosedChatKeepsItsCountForTheReleaseGrace() {
        val live = LiveSubagents()
        live.report("a", 4, 0)
        live.release("a", 5_000)
        assertEquals(4, live.snapshot()["a"])
        assertFalse(live.expire(5_000 + LiveSubagents.RELEASE_GRACE_MS - 1))
        assertTrue(live.expire(5_000 + LiveSubagents.RELEASE_GRACE_MS))
        assertTrue(live.snapshot().isEmpty())
    }

    @Test fun reopeningAChatInsideTheGraceKeepsTheCount() {
        val live = LiveSubagents()
        live.report("a", 4, 0)
        live.release("a", 1_000)
        live.report("a", 4, 2_000)
        assertNull(live.nextExpiry())
        assertEquals(4, live.snapshot()["a"])
    }

    @Test fun releasingAChatWithNoCountsOrReportingNoneForAFreshChatDoesNothing() {
        val live = LiveSubagents()
        live.release("x", 0)
        assertFalse(live.report("y", 0, 0))
        assertNull(live.nextExpiry())
        assertTrue(live.snapshot().isEmpty())
    }

    @Test fun chatsAreIndependent() {
        val live = LiveSubagents()
        live.report("a", 1, 0); live.report("b", 2, 0)
        live.release("a", 0)
        live.expire(LiveSubagents.RELEASE_GRACE_MS)
        assertEquals(mapOf("b" to 2), live.snapshot())
    }
}
