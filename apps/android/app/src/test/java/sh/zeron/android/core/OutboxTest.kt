package sh.zeron.android.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.Collections
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executor
import java.util.concurrent.Executors
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit

class OutboxTest {
    /** The main thread, run by the test: callbacks wait here until [drain]. */
    private class Main : Executor {
        val tasks = LinkedBlockingQueue<Runnable>()
        override fun execute(command: Runnable) {
            tasks.add(command)
        }

        fun drain(count: Int) = repeat(count) { tasks.poll(5, TimeUnit.SECONDS)!!.run() }
    }

    private val pool = Executors.newFixedThreadPool(4)

    @Test
    fun sends_land_in_the_order_they_were_typed() {
        val main = Main()
        val outbox = Outbox<Unit>(SerialLane(pool), main)
        val landed = Collections.synchronizedList(mutableListOf<String>())
        val first = CountDownLatch(1)
        // The first call is slow (a save of a large chat ahead of it); the rest must still wait for it.
        outbox.submit("one", Unit, call = {
            first.await(5, TimeUnit.SECONDS)
            landed += "one"
        })
        outbox.runInOrder { landed += "stop" }
        for (i in 2..20) outbox.submit("m$i", Unit, call = { landed += "m$i" })
        assertEquals(20, outbox.pending.size)
        first.countDown()
        main.drain(20)
        assertEquals(listOf("one", "stop") + (2..20).map { "m$it" }, landed)
        assertTrue(outbox.isIdle)
    }

    @Test
    fun a_send_shows_as_pending_until_its_call_returns() {
        val main = Main()
        val outbox = Outbox<Unit>(SerialLane(pool), main)
        val gate = CountDownLatch(1)
        var sent = false
        outbox.submit("hello", Unit, call = { gate.await(5, TimeUnit.SECONDS) }, sent = { sent = true })
        assertEquals(listOf("hello"), outbox.pending.map { it.text })
        gate.countDown()
        main.drain(1)
        assertTrue(sent)
        assertTrue(outbox.pending.isEmpty())
    }

    @Test
    fun a_failed_send_comes_back_with_everything_behind_it() {
        val main = Main()
        val outbox = Outbox<List<String>>(SerialLane(pool), main)
        val called = Collections.synchronizedList(mutableListOf<String>())
        var failed: List<String>? = null
        var error: String? = null
        outbox.onFailed = { items, e ->
            failed = items.map { it.text }
            error = e.message
        }
        val gate = CountDownLatch(1)
        outbox.submit("first", emptyList(), call = {
            gate.await(5, TimeUnit.SECONDS)
            called += "first"
        })
        outbox.submit("second", listOf("photo"), call = { throw IllegalStateException("host is too old") })
        outbox.submit("third", emptyList(), call = { called += "third" })
        gate.countDown()
        main.drain(2)
        // The third never went: it would have overtaken the second.
        assertEquals(listOf("first"), called.toList())
        assertEquals(listOf("second", "third"), failed)
        assertEquals("host is too old", error)
        assertTrue(outbox.isIdle)
    }

    @Test
    fun what_is_on_its_way_is_saved_until_the_core_has_it() {
        val main = Main()
        val saved = Collections.synchronizedList(mutableListOf<List<String>>())
        val outbox = Outbox<Unit>(SerialLane(pool), main, persist = { saved += it })
        val gate = CountDownLatch(1)
        outbox.submit("a", Unit, call = { gate.await(5, TimeUnit.SECONDS) })
        outbox.submit("b", Unit, call = { throw RuntimeException("offline") })
        assertEquals(listOf("a", "b"), saved.last())
        gate.countDown()
        main.drain(2)
        // Sent or given back to the draft: nothing is left to recover.
        assertEquals(listOf(listOf("a"), listOf("a", "b"), listOf("b"), emptyList()), saved.toList())
    }

    @Test
    fun a_failed_send_goes_back_ahead_of_what_was_typed_since() {
        assertEquals("first\n\nsecond\n\nnew text", restoredDraft(listOf("first", "second"), "new text"))
        assertEquals("only", restoredDraft(listOf("only"), ""))
        assertEquals("typed", restoredDraft(listOf(" "), "typed"))
        assertEquals("", restoredDraft(emptyList(), ""))
    }
}
