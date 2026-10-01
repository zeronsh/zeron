package sh.zeron.runtime

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.runInterruptible
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull
import java.util.Collections

/** Runs one-off guest commands (exec() and bootstrap steps). */
internal class GuestRunner(
    private val guest: Guest,
    private val store: StateStore,
    private val log: RuntimeLog,
) {
    private val live: MutableSet<Int> = Collections.synchronizedSet(HashSet())

    suspend fun run(
        argv: List<String>,
        asRoot: Boolean = false,
        timeoutMs: Long = 600_000,
        onLine: ((String) -> Unit)? = null,
    ): ExecResult = withContext(Dispatchers.IO) {
        val (process, pid) = guest.start(guest.command(argv, guest.env(store.secrets(), store.read().customServer), asRoot))
        live += pid
        val output = OutputBuffer(MAX_OUTPUT)
        // A plain thread: a blocking pipe read can't be cancelled, and a
        // daemonised grandchild may hold the pipe open after the command exits.
        val reader = Thread({
            try {
                process.inputStream.bufferedReader().useLines { lines ->
                    for (line in lines) {
                        output.append(line)
                        onLine?.invoke(line)
                    }
                }
            } catch (_: java.io.IOException) {
            }
        }, "zeron-guest-$pid").apply { isDaemon = true; start() }
        try {
            val exit = withTimeoutOrNull(timeoutMs) { runInterruptible { process.waitFor() } }
            if (exit == null) {
                ProcessTree.killTree(pid)
                runInterruptible { reader.join(2_000) }
                output.append("[zeron] timed out after ${timeoutMs / 1000}s")
                ExecResult(124, output.toString())
            } else {
                runInterruptible { reader.join(2_000) }
                ExecResult(exit, output.toString())
            }
        } finally {
            // Cancellation (reset(), a dead caller) must not leak guest trees.
            withContext(NonCancellable) {
                if (process.isAlive) ProcessTree.killTree(pid)
                live -= pid
            }
        }
    }

    /** Like [run], but a non-zero exit is an error carrying the output tail. */
    suspend fun runChecked(
        argv: List<String>,
        asRoot: Boolean = false,
        onLine: ((String) -> Unit)? = null,
    ): ExecResult {
        val result = run(argv, asRoot, timeoutMs = 30 * 60_000L) { line ->
            log.append(line)
            onLine?.invoke(line)
        }
        if (result.exitCode != 0) {
            throw GuestCommandException(
                "`${argv.joinToString(" ")}` exited with ${result.exitCode}",
                result.output.lines().takeLast(20).joinToString("\n"),
            )
        }
        return result
    }

    fun killAll() {
        for (pid in live.toList()) ProcessTree.killTree(pid)
    }

    private class OutputBuffer(private val max: Int) {
        private val sb = StringBuilder()

        @Synchronized
        fun append(line: String) {
            sb.append(line).append('\n')
            // Keep the tail: installers put the interesting part last.
            if (sb.length > max) sb.delete(0, sb.length - max)
        }

        @Synchronized
        override fun toString() = sb.toString().trimEnd('\n')
    }

    companion object {
        private const val MAX_OUTPUT = 512 * 1024
    }
}

internal class GuestCommandException(message: String, val outputTail: String) : Exception(message)
