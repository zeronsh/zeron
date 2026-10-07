package sh.zeron.android.schedule

import android.app.Application
import androidx.test.core.app.ApplicationProvider
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.async
import android.os.Looper
import java.time.Duration
import org.robolectric.Shadows.shadowOf
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import sh.zeron.android.core.CoreConnect
import sh.zeron.android.screenshots.Screenshots

/**
 * A scheduled first message from the New Session screen, delivered against
 * the Demo workspace (host build of the core): the session is created at
 * fire time with the saved CLI / model and gets the message.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34])
@OptIn(kotlinx.coroutines.DelicateCoroutinesApi::class)
class ScheduledNewSessionTest {
    private val app = ApplicationProvider.getApplicationContext<Application>()

    @Before
    fun gate() = Screenshots.assumeHostCore()

    @Test
    fun createsTheSessionThenSends() = runBlocking {
        val probe = CoreConnect.open(app, "demo")
        var projects = probe.projects()
        val until = System.currentTimeMillis() + 10_000
        while (projects.isEmpty() && System.currentTimeMillis() < until) {
            kotlinx.coroutines.delay(100)
            projects = probe.projects()
        }
        val project = projects.first()
        runCatching { probe.shutdown() }
        probe.close()

        val spec = NewSessionSpec(projectId = project.id, harness = "codex", label = project.name)
        val m = ScheduledMessage(workspace = "demo", chatId = "", text = "scheduled hello", atMs = System.currentTimeMillis(), newSession = spec)
        ScheduledStore(app).add(m)
        // The sender's deadlines run on SystemClock, which Robolectric only
        // moves while the main looper idles: deliver off-thread, tick here.
        val job = kotlinx.coroutines.GlobalScope.async(kotlinx.coroutines.Dispatchers.Default) { ScheduledSender(app).deliver(m.id, budgetMs = 20_000) }
        val stop = System.currentTimeMillis() + 60_000
        while (!job.isCompleted && System.currentTimeMillis() < stop) {
            shadowOf(Looper.getMainLooper()).idleFor(Duration.ofMillis(200))
            Thread.sleep(20)
        }
        check(job.isCompleted) { "delivery didn't finish" }
        val result = job.await()
        assertTrue("got $result", result is ScheduledSender.Result.Sent || result is ScheduledSender.Result.Pending)
        val sent = when (result) {
            is ScheduledSender.Result.Sent -> result.message
            is ScheduledSender.Result.Pending -> result.message
            else -> error("unreachable")
        }
        assertTrue(sent.chatId.isNotEmpty())
        assertEquals(emptyList<ScheduledMessage>(), ScheduledStore(app).list())
    }
}
