package sh.zeron.android.update

import android.app.Application
import androidx.test.core.app.ApplicationProvider
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.currentTime
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import sh.zeron.android.core.Updater

/** The automatic update check's 30-minute cadence and its foreground loop. */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34])
class UpdateScheduleTest {
    private val app get() = ApplicationProvider.getApplicationContext<Application>()
    private val min = 60_000L
    private val now = 1_800_000_000_000L

    @Before fun clear() {
        app.getSharedPreferences("zeron-update", 0).edit().clear().commit()
    }

    @Test fun cadenceIsThirtyMinutes() {
        assertEquals(30 * min, Updater.QUIET_MS)
    }

    @Test fun neverCheckedIsDue() {
        val u = Updater(app)
        assertTrue(u.dueForQuietCheck(now))
        assertEquals(0L, u.msUntilQuietCheck(now))
    }

    @Test fun dueThirtyMinutesAfterASuccessfulCheck() {
        val u = Updater(app)
        u.lastCheckMs = now
        assertFalse(u.dueForQuietCheck(now + 29 * min))
        assertEquals(10 * min, u.msUntilQuietCheck(now + 20 * min))
        assertTrue(u.dueForQuietCheck(now + 30 * min))
        assertTrue(u.dueForQuietCheck(now + 6 * 60 * min))
    }

    @Test fun aFailedAttemptWaitsTheSameThirtyMinutes() {
        // lastCheck stays old after a failure; the attempt stamp holds retries off.
        val u = Updater(app)
        u.lastCheckMs = now - 5 * 60 * min
        u.lastAttemptMs = now
        assertFalse(u.dueForQuietCheck(now + 1 * min))
        assertFalse(u.dueForQuietCheck(now + 29 * min))
        assertTrue(u.dueForQuietCheck(now + 30 * min))
    }

    @Test fun aClockSetBackDoesNotBlockChecks() {
        val u = Updater(app)
        u.lastCheckMs = now + 24 * 60 * min
        assertTrue(u.dueForQuietCheck(now))
    }

    @OptIn(ExperimentalCoroutinesApi::class)
    @Test fun foregroundLoopChecksEveryThirtyMinutesUntilCancelled() = runTest {
        var last = 0L
        val checks = mutableListOf<Long>()
        val loop = launch {
            Updater.repeatQuietChecks(
                nextInMs = { Updater.quietWaitMs(now + currentTime, last) },
                check = {
                    // Like ZeronModel: stamp the attempt only when one is due.
                    if (Updater.quietWaitMs(now + currentTime, last) == 0L) { last = now + currentTime; checks += currentTime }
                },
            )
        }
        runCurrent()
        assertEquals(listOf(0L), checks)
        advanceTimeBy(29 * min)
        assertEquals(1, checks.size)
        advanceTimeBy(2 * min)
        assertEquals(listOf(0L, 30 * min), checks)
        advanceTimeBy(60 * min)
        assertEquals(listOf(0L, 30 * min, 60 * min, 90 * min), checks)
        loop.cancel()
        advanceTimeBy(120 * min)
        assertEquals(4, checks.size)
    }

    @OptIn(ExperimentalCoroutinesApi::class)
    @Test fun loopNeverSpinsWhenNothingIsDue() = runTest {
        var calls = 0
        val loop = launch { Updater.repeatQuietChecks(nextInMs = { 0L }, check = { calls++ }) }
        advanceTimeBy(10 * min - 1)
        assertEquals(10, calls)
        loop.cancel()
    }
}
