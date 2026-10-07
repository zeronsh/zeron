package sh.zeron.android.schedule

import android.app.AlarmManager
import android.app.Application
import androidx.test.core.app.ApplicationProvider
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34])
class ScheduledAlarmsTest {
    private val app = ApplicationProvider.getApplicationContext<Application>()
    private val alarms = shadowOf(app.getSystemService(AlarmManager::class.java))

    private fun message(id: String, inMs: Long) = ScheduledMessage(
        id = id,
        workspace = "machine-1",
        chatId = "chat-1",
        text = "run the nightly build",
        atMs = System.currentTimeMillis() + inMs,
        chatTitle = "Nightly",
    )

    @Test
    fun scheduleStoresAndSetsAnExactWakeupAlarm() {
        org.robolectric.shadows.ShadowAlarmManager.setCanScheduleExactAlarms(true)
        val m = message("a", 3_600_000)
        assertTrue(ScheduledAlarms.schedule(app, m))
        assertEquals(listOf(m), ScheduledStore(app).list())
        val alarm = alarms.scheduledAlarms.single()
        assertEquals(AlarmManager.RTC_WAKEUP, alarm.type)
        assertEquals(m.atMs, alarm.triggerAtMs)
    }

    @Test
    fun withoutExactPermissionItStillSetsAnInexactAlarm() {
        org.robolectric.shadows.ShadowAlarmManager.setCanScheduleExactAlarms(false)
        val m = message("a", 3_600_000)
        assertEquals(false, ScheduledAlarms.schedule(app, m))
        assertEquals(m.atMs, alarms.scheduledAlarms.single().triggerAtMs)
    }

    @Test
    fun cancelRemovesTheMessageAndTheAlarm() {
        ScheduledAlarms.schedule(app, message("a", 3_600_000))
        ScheduledAlarms.schedule(app, message("b", 7_200_000))
        ScheduledAlarms.cancel(app, "a")
        assertEquals(listOf("b"), ScheduledStore(app).list().map { it.id })
        assertEquals(1, alarms.scheduledAlarms.size)
    }

    @Test
    fun takeClaimsOnce() {
        val store = ScheduledStore(app)
        store.add(message("a", 1_000))
        assertEquals("a", store.take("a")?.id)
        assertNull(store.take("a"))
    }

    @Test
    fun restoreRearmsAndMissedOnesFireSoon() {
        val store = ScheduledStore(app)
        store.add(message("late", -60_000))
        store.add(message("later", 60_000))
        ScheduledAlarms.restoreAll(app)
        val times = alarms.scheduledAlarms.map { it.triggerAtMs }.sorted()
        assertEquals(2, times.size)
        assertTrue(times[0] >= System.currentTimeMillis())
    }

    @Test
    fun newSessionSpecSurvivesTheStore() {
        val spec = NewSessionSpec(
            projectId = "p1", harness = "codex", model = "gpt-5", effort = "high",
            branch = "main", worktree = true, projectPath = "/src/zeron", label = "zeron",
        )
        val m = message("n", 60_000).copy(chatId = "", newSession = spec)
        ScheduledStore(app).add(m)
        assertEquals(m, ScheduledStore(app).get("n"))
        // Old entries (no newSession key) still read.
        val plain = message("p", 60_000)
        assertNull(ScheduledMessage.fromJson(plain.toJson()).newSession)
    }
}
