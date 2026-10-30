package sh.zeron.android.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Test
import java.util.concurrent.Executor

class SnapshotSchedulerTest {
    /** Runs nothing until the test says so. */
    private class Queue : Executor {
        val tasks = ArrayDeque<Runnable>()
        override fun execute(command: Runnable) { tasks.addLast(command) }
        fun runNext() = tasks.removeFirst().run()
        fun runAll() { while (tasks.isNotEmpty()) runNext() }
    }

    private class Client(var revision: ULong) {
        var builds = 0
        var duringBuild: () -> Unit = {}
        var fail = false
    }

    private class Snapshot(val revision: ULong, val by: Client)

    private val background = Queue()
    private val main = Queue()
    private var current: Client? = null
    private var published: Snapshot? = null
    private var publishedInBackground = 0

    private val scheduler = SnapshotScheduler<Client, Snapshot>(
        background = background,
        main = main,
        currentClient = { current },
        published = { published },
        setPublished = { published = it },
        revisionOf = { it.revision },
        clientRevision = { it.revision },
        build = { c ->
            val revision = c.revision
            c.builds++
            c.duringBuild()
            if (c.fail) error("build failed")
            Snapshot(revision, c)
        },
    )

    private fun client(revision: ULong) = Client(revision).also { current = it }
    private fun schedule(c: Client) = scheduler.schedule(c) { publishedInBackground++ }

    @Test
    fun refreshPublishesBeforeReturning() {
        val c = client(1u)
        scheduler.refresh(c)
        assertEquals(1uL, published?.revision)
        assertEquals(0, background.tasks.size + main.tasks.size)
    }

    @Test
    fun scheduledBuildRunsInTheBackgroundAndPublishesOnMain() {
        val c = client(1u)
        schedule(c)
        assertEquals(0, c.builds)
        background.runNext()
        assertEquals(1, c.builds)
        assertNull(published)
        main.runAll()
        assertEquals(1uL, published?.revision)
        assertEquals(1, publishedInBackground)
    }

    @Test
    fun burstsCoalesceToOneRunningAndOneQueued() {
        val c = client(1u)
        repeat(3) { schedule(c) }
        assertEquals(1, background.tasks.size)
        // Frames that arrive while a build runs queue exactly one more.
        c.duringBuild = { c.duringBuild = {}; c.revision = 2u; repeat(3) { schedule(c) } }
        background.runNext()
        assertEquals(1, background.tasks.size)
        main.runAll()
        background.runAll()
        main.runAll()
        assertEquals(2, c.builds)
        assertEquals(2uL, published?.revision)
    }

    @Test
    fun unchangedRevisionSkipsTheBuild() {
        val c = client(1u)
        scheduler.refresh(c)
        scheduler.refresh(c)
        schedule(c)
        background.runAll()
        assertEquals(1, c.builds)
        assertEquals(0, main.tasks.size)
        c.revision = 2u
        scheduler.refresh(c)
        assertEquals(2, c.builds)
        assertEquals(2uL, published?.revision)
    }

    @Test
    fun staleBuildFinishingAfterAFresherOneIsDropped() {
        val c = client(1u)
        schedule(c)
        background.runNext()
        c.revision = 2u
        scheduler.refresh(c)
        main.runAll()
        assertEquals(2uL, published?.revision)
        assertEquals(0, publishedInBackground)
    }

    @Test
    fun buildOfTheAlreadyPublishedRevisionIsNotRepublished() {
        val c = client(1u)
        schedule(c)
        background.runNext()
        scheduler.refresh(c)
        val fromRefresh = published
        main.runAll()
        assertSame(fromRefresh, published)
        assertEquals(0, publishedInBackground)
    }

    @Test
    fun buildFromAClientReplacedMidBuildIsDropped() {
        val old = client(5u)
        schedule(old)
        background.runNext()
        val replacement = client(1u)
        scheduler.refresh(replacement)
        main.runAll()
        // Dropped even though its revision is higher: revisions are per client.
        assertSame(replacement, published?.by)
        assertEquals(0, publishedInBackground)
    }

    @Test
    fun newClientIsBuiltEvenAtThePublishedRevision() {
        scheduler.refresh(client(5u))
        val replacement = client(5u)
        scheduler.refresh(replacement)
        assertEquals(1, replacement.builds)
        assertSame(replacement, published?.by)
    }

    @Test
    fun failedBackgroundBuildPublishesNothingAndLaterFramesStillBuild() {
        val c = client(1u)
        c.fail = true
        schedule(c)
        background.runAll()
        assertEquals(0, main.tasks.size)
        c.fail = false
        schedule(c)
        background.runAll()
        main.runAll()
        assertEquals(1uL, published?.revision)
    }
}
