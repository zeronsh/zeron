package sh.zeron.android.ui

import org.junit.Assert.assertEquals
import org.junit.Test

/** Mirrors the `relative_time_label` tests in crates/client/src/workspace/view.rs. */
class RelativeTimeTest {
    private val now = 1_700_000_000_000L

    @Test
    fun underAMinuteIsNow() {
        assertEquals("now", RelativeTime.label(now, now))
        assertEquals("now", RelativeTime.label(now - 59_000, now))
    }

    @Test
    fun minutes() {
        assertEquals("1m", RelativeTime.label(now - 60_000, now))
        assertEquals("34m", RelativeTime.label(now - 34 * 60_000, now))
        assertEquals("59m", RelativeTime.label(now - 3_599_000, now))
    }

    @Test
    fun hours() {
        assertEquals("1h", RelativeTime.label(now - 3_600_000, now))
        assertEquals("4h", RelativeTime.label(now - 4 * 3_600_000 - 1, now))
        assertEquals("23h", RelativeTime.label(now - 86_399_000, now))
    }

    @Test
    fun days() {
        assertEquals("1d", RelativeTime.label(now - 86_400_000, now))
        assertEquals("2d", RelativeTime.label(now - 2 * 86_400_000, now))
    }

    @Test
    fun futureIsNow() {
        assertEquals("now", RelativeTime.label(now + 5_000, now))
    }
}
