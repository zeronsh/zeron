package sh.zeron.android.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import sh.zeron.android.R
import sh.zeron.android.design.MarkKind
import sh.zeron.android.design.ZeronDark
import uniffi.zeron_core.ChatIndicator
import uniffi.zeron_core.SendState

/**
 * Home-row status glyphs: icon-only, the running dot-matrix alone, and a
 * done check / failed dot that stays after the chat has been seen (the core's
 * `indicator` drops back to IDLE once seen on any device; `lastOutcome` keeps
 * how the last run ended).
 */
class RowStatusCornerTest {
    private val c = ZeronDark

    @Test
    fun runningIsTheDotMatrixAlone() {
        val corner = statusCorner(null, ChatIndicator.WORKING, ChatIndicator.WORKING, c)!!
        assertEquals(MarkKind.Spinner, corner.mark)
        assertEquals(R.string.status_working, corner.word)
        assertFalse(corner.showTime)
        // A new run over an errored one reads running, not failed.
        assertEquals(MarkKind.Spinner, statusCorner(null, ChatIndicator.WORKING, ChatIndicator.ERRORED, c)!!.mark)
    }

    @Test
    fun awaitingInputIsAnInputDot() {
        val corner = statusCorner(null, ChatIndicator.AWAITING_INPUT, ChatIndicator.AWAITING_INPUT, c)!!
        assertEquals(MarkKind.Dot(c.input), corner.mark)
        assertFalse(corner.showTime)
    }

    @Test
    fun doneAndFailedSurviveBeingSeen() {
        // Seen: indicator IDLE, outcome still says how it ended.
        val done = statusCorner(null, ChatIndicator.IDLE, ChatIndicator.COMPLETED, c)!!
        assertEquals(MarkKind.Check(c.done), done.mark)
        assertEquals(R.string.status_done, done.word)
        assertTrue(done.showTime)
        val failed = statusCorner(null, ChatIndicator.IDLE, ChatIndicator.ERRORED, c)!!
        assertEquals(MarkKind.Dot(c.failed), failed.mark)
        assertEquals(R.string.status_failed, failed.word)
        assertTrue(failed.showTime)
        // Unseen reads the same glyphs.
        assertEquals(done, statusCorner(null, ChatIndicator.COMPLETED, ChatIndicator.COMPLETED, c))
        assertEquals(failed, statusCorner(null, ChatIndicator.ERRORED, ChatIndicator.ERRORED, c))
    }

    @Test
    fun failedSendWinsAndNeverRanShowsTimeOnly() {
        val corner = statusCorner(SendState.FAILED, ChatIndicator.WORKING, ChatIndicator.COMPLETED, c)!!
        assertEquals(MarkKind.Dot(c.danger), corner.mark)
        assertNull(statusCorner(null, ChatIndicator.IDLE, ChatIndicator.IDLE, c))
    }
}
