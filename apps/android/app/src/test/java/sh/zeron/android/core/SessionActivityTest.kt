package sh.zeron.android.core

import org.junit.Assert.*
import org.junit.Test
import sh.zeron.android.ui.SessionWorkingFilterTestRows
import uniffi.zeron_core.ChatIndicator

class SessionActivityTest {
    @Test fun mainRunningWinsEvenWithChildrenAndCallbacks() {
        assertEquals(SessionActivity.Shape.MainRunning, SessionActivity.shape(ChatIndicator.WORKING, 4u, 2u))
        assertEquals(SessionActivity.Shape.MainRunning, SessionActivity.shape(ChatIndicator.WORKING, 0u, 0u))
    }

    @Test fun idleAndUnseenCompletedParentsShowYellowUntilTheLastChildSettles() {
        for (status in listOf(ChatIndicator.IDLE, ChatIndicator.COMPLETED)) {
            assertEquals(SessionActivity.Shape.SubagentsRunning, SessionActivity.shape(status, 1u, 1u))
            assertNull(SessionActivity.shape(status, 0u, 0u))
        }
    }

    @Test fun blueRequiresAnExplicitCallbackAndNoRunningChildren() {
        assertEquals(SessionActivity.Shape.CallbackWaiting, SessionActivity.shape(ChatIndicator.IDLE, 0u, 1u))
        assertEquals(SessionActivity.Shape.SubagentsRunning, SessionActivity.shape(ChatIndicator.IDLE, 2u, 1u))
        assertNull(SessionActivity.shape(ChatIndicator.IDLE, 0u, 0u))
    }

    @Test fun inputAndFailureKeepTheirAttentionLabelsWhileChildrenCanStillBeWorking() {
        for (status in listOf(ChatIndicator.AWAITING_INPUT, ChatIndicator.ERRORED)) {
            assertNull(SessionActivity.shape(status, 3u, 1u))
            assertTrue(SessionActivity.isWorking(status, 3u, 0u))
            assertFalse(SessionActivity.isWorking(status, 0u, 0u))
        }
    }

    @Test fun workingIncludesMainChildrenAndConfirmedCallbacksButNotAnOrdinaryIdleChat() {
        assertTrue(SessionActivity.isWorking(ChatIndicator.WORKING, 0u, 0u))
        assertTrue(SessionActivity.isWorking(ChatIndicator.IDLE, 1u, 0u))
        assertTrue(SessionActivity.isWorking(ChatIndicator.COMPLETED, 0u, 1u))
        assertFalse(SessionActivity.isWorking(ChatIndicator.IDLE, 0u, 0u))
    }

    @Test fun badgeHasNoTextForZeroOrOverflowIcon() {
        assertNull(SessionActivity.badgeLabel(0u))
        assertEquals("1", SessionActivity.badgeLabel(1u))
        assertEquals("9", SessionActivity.badgeLabel(9u))
        assertEquals("10", SessionActivity.badgeLabel(10u))
        assertEquals("99", SessionActivity.badgeLabel(99u))
        assertNull(SessionActivity.badgeLabel(100u))
        assertNull(SessionActivity.badgeLabel(UInt.MAX_VALUE))
    }

    @Test fun mergedCountIsTheLargerOfThePublishedCountAndWhatTheOpenChatShows() {
        // An older engine publishes nothing: the open chat's own chips decide.
        assertEquals(1u, SessionActivity.mergedSubagents(0u, 1))
        // A published count the chat has not synced yet still shows.
        assertEquals(3u, SessionActivity.mergedSubagents(3u, 0))
        assertEquals(3u, SessionActivity.mergedSubagents(3u, null))
        // Never lower than what the thread plainly shows, nor than the row.
        assertEquals(4u, SessionActivity.mergedSubagents(2u, 4))
        assertEquals(4u, SessionActivity.mergedSubagents(4u, 2))
        assertEquals(0u, SessionActivity.mergedSubagents(0u, null))
        assertEquals(0u, SessionActivity.mergedSubagents(0u, -3))
        assertEquals(Int.MAX_VALUE.toUInt(), SessionActivity.mergedSubagents(0u, Int.MAX_VALUE))
    }

    @Test fun mergedRowOnlyChangesTheCountAndDrivesTheYellowShape() {
        val row = SessionWorkingFilterTestRows.row("a")
        val merged = SessionActivity.merged(row, mapOf("a" to 2))
        assertEquals(2u, merged.runningSubagents)
        assertEquals(row.copy(runningSubagents = 2u), merged)
        assertEquals(SessionActivity.Shape.SubagentsRunning, SessionActivity.shape(merged.indicator, merged.runningSubagents, merged.pendingCallbacks))
        assertTrue(SessionActivity.isWorking(merged))
        // Other chats, and an empty map, return the very same row.
        assertSame(row, SessionActivity.merged(row, mapOf("b" to 5)))
        assertSame(row, SessionActivity.merged(row, emptyMap()))
        val rows = listOf(row, SessionWorkingFilterTestRows.row("b"))
        assertSame(rows, SessionActivity.merged(rows, emptyMap()))
        assertEquals(listOf(2u, 0u), SessionActivity.merged(rows, mapOf("a" to 2)).map { it.runningSubagents })
    }

    @Test fun mainRunningStillWinsTheShapeOverMergedChildren() {
        val row = SessionWorkingFilterTestRows.row("a", ChatIndicator.WORKING)
        val merged = SessionActivity.merged(row, mapOf("a" to 3))
        assertEquals(SessionActivity.Shape.MainRunning, SessionActivity.shape(merged.indicator, merged.runningSubagents, merged.pendingCallbacks))
        assertEquals(3u, merged.runningSubagents)
    }
}
