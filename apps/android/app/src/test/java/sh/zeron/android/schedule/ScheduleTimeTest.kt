package sh.zeron.android.schedule

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.Calendar
import java.util.TimeZone

class ScheduleTimeTest {
    private val taipei = TimeZone.getTimeZone("Asia/Taipei")

    private fun at(y: Int, mo: Int, d: Int, h: Int, mi: Int, s: Int = 0): Long =
        Calendar.getInstance(taipei).apply {
            clear()
            set(y, mo - 1, d, h, mi, s)
        }.timeInMillis

    @Test
    fun laterTodayStaysToday() {
        val now = at(2026, 9, 29, 23, 30)
        assertEquals(at(2026, 9, 29, 23, 45), ScheduleTime.next(23, 45, now, taipei))
    }

    @Test
    fun passedTimeMovesToTomorrow() {
        val now = at(2026, 9, 29, 23, 30)
        val next = ScheduleTime.next(1, 20, now, taipei)
        assertEquals(at(2026, 9, 30, 1, 20), next)
        assertFalse(ScheduleTime.isSameDay(now, next, taipei))
    }

    @Test
    fun theCurrentMinuteCountsAsPassed() {
        val now = at(2026, 9, 29, 23, 30, 10)
        assertEquals(at(2026, 9, 30, 23, 30), ScheduleTime.next(23, 30, now, taipei))
    }

    @Test
    fun rollsOverMonthEnd() {
        val now = at(2026, 9, 30, 22, 0)
        assertEquals(at(2026, 10, 1, 8, 0), ScheduleTime.next(8, 0, now, taipei))
    }

    @Test
    fun chipClockIs24Hour() {
        assertEquals("01:20", ScheduleTime.clock(at(2026, 9, 30, 1, 20), taipei))
        assertEquals("13:05", ScheduleTime.clock(at(2026, 9, 30, 13, 5), taipei))
        assertTrue(ScheduleTime.isSameDay(at(2026, 9, 30, 0, 0), at(2026, 9, 30, 23, 59), taipei))
    }

    @Test
    fun afterAddsTheDurationToNow() {
        val now = at(2026, 9, 30, 13, 5, 42)
        assertEquals(now + 15 * 60_000L, ScheduleTime.after(15, now))
        assertEquals(at(2026, 9, 30, 14, 35, 42), ScheduleTime.after(90, now))
        assertEquals(now, ScheduleTime.after(-5, now))
    }

    @Test
    fun afterCrossesMidnightIntoTomorrow() {
        val now = at(2026, 9, 30, 23, 20)
        val fire = ScheduleTime.after(2 * 60, now)
        assertEquals(at(2026, 10, 1, 1, 20), fire)
        assertEquals(1, ScheduleTime.daysFrom(fire, now, taipei))
        assertEquals("01:20", ScheduleTime.clock(fire, taipei))
    }

    @Test
    fun daysFromCountsCalendarDays() {
        val now = at(2026, 9, 30, 23, 59)
        assertEquals(0, ScheduleTime.daysFrom(at(2026, 9, 30, 0, 1), now, taipei))
        assertEquals(1, ScheduleTime.daysFrom(at(2026, 10, 1, 0, 0), now, taipei))
        assertEquals(2, ScheduleTime.daysFrom(at(2026, 10, 2, 23, 0), now, taipei))
        // DST zones: still whole days.
        val ny = TimeZone.getTimeZone("America/New_York")
        val before = Calendar.getInstance(ny).apply { clear(); set(2026, 10, 1, 0, 30) }.timeInMillis
        val after = Calendar.getInstance(ny).apply { clear(); set(2026, 10, 2, 0, 30) }.timeInMillis
        assertEquals(1, ScheduleTime.daysFrom(after, before, ny))
    }
}
