package sh.zeron.android.core

import java.util.ArrayDeque
import java.util.concurrent.Executor
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicInteger

/**
 * Runs one chat's tasks one at a time, in the order given, on a shared pool
 * (the `SerialExecutor` of `Executor`'s docs). An idle lane holds no thread.
 */
class SerialLane(private val pool: Executor) : Executor {
    private val tasks = ArrayDeque<Runnable>()
    private var active: Runnable? = null

    @Synchronized
    override fun execute(task: Runnable) {
        tasks.add(Runnable {
            try {
                task.run()
            } finally {
                next()
            }
        })
        if (active == null) next()
    }

    @Synchronized
    private fun next() {
        active = tasks.poll()
        active?.let(pool::execute)
    }

    companion object {
        private val count = AtomicInteger()

        /** Where every chat's lane runs: the core's calls block on its storage, never on the UI. */
        val pool: Executor = Executors.newCachedThreadPool { r ->
            Thread(r, "zeron-send-${count.incrementAndGet()}").apply { isDaemon = true }
        }
    }
}

/**
 * One chat's messages on their way into the core. `SessionHandle.send` writes
 * the chat's documents and storage, so it can wait behind a save of a large
 * chat: it runs on the chat's [SerialLane], never on the main thread, and two
 * quick sends land in the order they were typed (a stop before a send stays
 * before it).
 *
 * Until the call returns, the message shows as [pending] (the composer draws
 * it at once). A send that fails takes every send queued after it back with
 * it, so a later message never overtakes an earlier one that didn't go.
 *
 * `T` is what rides along with the text (the composer's staged images).
 * [persist] records the texts not yet in the core, so a process that dies
 * mid-send keeps them as a draft; it's called (under a lock, so keep it quick)
 * when one is added and on the lane as soon as a call returns.
 */
class Outbox<T>(
    private val lane: Executor,
    private val main: Executor,
    private val persist: (List<String>) -> Unit = {},
) {
    class Pending<T>(val id: Long, val text: String, val attachments: T) {
        @Volatile internal var cancelled = false
    }

    private val lock = Any()
    private val queue = ArrayList<Pending<T>>()
    private var nextId = 0L

    /** Main thread: the pending list changed. */
    var onChange: () -> Unit = {}

    /**
     * Main thread: these sends didn't go (the first one failed with the error;
     * the rest were behind it), oldest first. Their text belongs back in the draft.
     */
    var onFailed: (List<Pending<T>>, Exception) -> Unit = { _, _ -> }

    /** What hasn't reached the core yet, oldest first. */
    val pending: List<Pending<T>> get() = synchronized(lock) { queue.toList() }

    val isIdle: Boolean get() = synchronized(lock) { queue.isEmpty() }

    /**
     * Queue a message: shown as pending now, `call` runs on the lane after
     * everything submitted before it. `sent` runs on the main thread once it
     * returned.
     */
    fun submit(text: String, attachments: T, call: () -> Unit, sent: () -> Unit = {}): Pending<T> {
        val item = synchronized(lock) {
            Pending(nextId++, text, attachments).also {
                queue.add(it)
                persistLocked()
            }
        }
        onChange()
        lane.execute {
            if (item.cancelled) return@execute
            val failure = try {
                call()
                null
            } catch (e: Exception) {
                e
            }
            val dropped = synchronized(lock) {
                val gone = if (failure == null) {
                    queue.remove(item)
                    emptyList()
                } else {
                    // This one and everything queued behind it: none of them has started.
                    val from = queue.indexOf(item).coerceAtLeast(0)
                    queue.subList(from, queue.size).toList().also { rest ->
                        rest.forEach { it.cancelled = true }
                        queue.removeAll(rest.toSet())
                    }
                }
                persistLocked()
                gone
            }
            main.execute {
                onChange()
                if (failure == null) sent() else onFailed(dropped, failure)
            }
        }
        return item
    }

    /** Run a call with no message of its own (a stop, an answer) in order with the sends. */
    fun runInOrder(call: () -> Unit) {
        lane.execute { runCatching(call) }
    }

    // Under the lock, so a stale list never lands after a newer one.
    private fun persistLocked() = persist(queue.map { it.text })
}

/** Texts that didn't go, back ahead of whatever was typed since (as the composer would have had them). */
fun restoredDraft(unsent: List<String>, current: String): String =
    (unsent + current).filter { it.isNotBlank() }.joinToString("\n\n")
