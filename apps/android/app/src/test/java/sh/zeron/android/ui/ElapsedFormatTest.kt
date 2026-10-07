package sh.zeron.android.ui

import org.junit.Assert.assertEquals
import org.junit.Test

class ElapsedFormatTest {
    @Test
    fun seconds() {
        assertEquals("0s", ElapsedFormat.format(0))
        assertEquals("1s", ElapsedFormat.format(1))
        assertEquals("59s", ElapsedFormat.format(59))
    }

    @Test
    fun minutes() {
        assertEquals("1m 0s", ElapsedFormat.format(60))
        assertEquals("1m 5s", ElapsedFormat.format(65))
        assertEquals("59m 59s", ElapsedFormat.format(3599))
    }

    @Test
    fun hours() {
        assertEquals("1h 0m", ElapsedFormat.format(3600))
        assertEquals("1h 2m", ElapsedFormat.format(3725))
        assertEquals("23h 59m", ElapsedFormat.format(86399))
    }

    @Test
    fun days() {
        assertEquals("1d 0h", ElapsedFormat.format(86400))
        assertEquals("1d 1h", ElapsedFormat.format(90061))
    }

    @Test
    fun negativeClampsToZero() {
        assertEquals("0s", ElapsedFormat.format(-1))
        assertEquals("0s", ElapsedFormat.format(-7200))
    }
}
