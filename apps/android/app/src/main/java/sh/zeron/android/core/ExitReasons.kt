package sh.zeron.android.core

import android.app.ActivityManager
import android.app.ApplicationExitInfo
import android.content.Context
import android.os.Build
import android.util.Log
import java.io.File
import java.io.InputStream
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import java.util.TimeZone

/**
 * Why earlier runs of the app ended (Android 11+): an ANR, a crash, the
 * system reclaiming memory. Logged once each at startup, with a bounded
 * excerpt of the ANR or native crash trace, so a later report says how the
 * app died, not only that it did. Goes to logcat and to `exits.log` in the
 * app's files (the last [FILE_LIMIT] bytes).
 */
object ExitReasons {
    /** What [describe] needs from an `ApplicationExitInfo` (plain, so it can be tested off-device). */
    data class Exit(
        val timestamp: Long,
        val reason: Int,
        val status: Int,
        val importance: Int,
        val pssKb: Long,
        val rssKb: Long,
        val description: String?,
        val processName: String?,
    )

    private const val HISTORY = 16
    private const val PREFS = "exit-reasons"
    private const val NEWEST = "newest"
    const val FILE_NAME = "exits.log"
    private const val FILE_LIMIT = 256 * 1024
    const val TRACE_LINES = 200
    const val TRACE_BYTES = 32 * 1024

    /** Off the main thread: reading the history and its traces is disk work. */
    fun recordInBackground(context: Context) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) return
        val app = context.applicationContext
        Thread({ runCatching { record(app) }.onFailure { Log.w("Zeron", "exit reasons unavailable", it) } }, "zeron-exit-reasons").start()
    }

    private fun record(context: Context) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) return
        val am = context.getSystemService(ActivityManager::class.java) ?: return
        val infos = am.getHistoricalProcessExitReasons(context.packageName, 0, HISTORY)
        val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        val byExit = infos.associateBy {
            Exit(it.timestamp, it.reason, it.status, it.importance, it.pss, it.rss, it.description, it.processName)
        }
        val newest = prefs.getLong(NEWEST, 0L)
        val fresh = unseen(byExit.keys, newest)
        if (fresh.isEmpty()) return
        val out = StringBuilder()
        for (exit in fresh) {
            val line = describe(exit)
            Log.w("Zeron", line)
            out.append(line).append('\n')
            if (exit.reason == ApplicationExitInfo.REASON_ANR || exit.reason == ApplicationExitInfo.REASON_CRASH_NATIVE) {
                val trace = runCatching { byExit[exit]?.traceInputStream?.use { excerpt(it) } }.getOrNull()
                if (!trace.isNullOrEmpty()) {
                    Log.w("Zeron", "exit trace:\n$trace")
                    out.append(trace.prependIndent("  ")).append('\n')
                }
            }
        }
        append(File(context.filesDir, FILE_NAME), out.toString())
        prefs.edit().putLong(NEWEST, fresh.maxOf { it.timestamp }).apply()
    }

    /** Exits after the newest one already logged, oldest first. */
    fun unseen(exits: Collection<Exit>, newest: Long): List<Exit> =
        exits.filter { it.timestamp > newest }.sortedBy { it.timestamp }

    fun describe(exit: Exit): String {
        val time = SimpleDateFormat("yyyy-MM-dd'T'HH:mm:ss.SSS'Z'", Locale.US).apply { timeZone = TimeZone.getTimeZone("UTC") }
        return buildString {
            append("previous exit ").append(time.format(Date(exit.timestamp)))
            append(" reason=").append(reasonName(exit.reason))
            append(" status=").append(exit.status)
            append(" importance=").append(exit.importance)
            append(" pss=").append(exit.pssKb).append("kB")
            append(" rss=").append(exit.rssKb).append("kB")
            exit.processName?.let { append(" process=").append(it) }
            exit.description?.takeIf { it.isNotBlank() }?.let { append(" description=\"").append(it.replace('\n', ' ')).append('"') }
        }
    }

    /** `ApplicationExitInfo.REASON_*` by value (the newer ones too, whatever this build compiles against). */
    fun reasonName(reason: Int): String = when (reason) {
        0 -> "UNKNOWN"
        1 -> "EXIT_SELF"
        2 -> "SIGNALED"
        3 -> "LOW_MEMORY"
        4 -> "CRASH"
        5 -> "CRASH_NATIVE"
        6 -> "ANR"
        7 -> "INITIALIZATION_FAILURE"
        8 -> "PERMISSION_CHANGE"
        9 -> "EXCESSIVE_RESOURCE_USAGE"
        10 -> "USER_REQUESTED"
        11 -> "USER_STOPPED"
        12 -> "DEPENDENCY_DIED"
        13 -> "OTHER"
        14 -> "FREEZER"
        15 -> "PACKAGE_STATE_CHANGE"
        16 -> "PACKAGE_UPDATED"
        else -> "REASON_$reason"
    }

    /**
     * The start of a trace: the main thread's section when there is one (an
     * ANR dump lists every thread; the main one is what hung), else the top
     * (a native tombstone). At most [maxLines] lines and about [maxBytes], and
     * no more than [scanLimit] characters are read looking for the main thread.
     */
    fun excerpt(
        stream: InputStream,
        maxLines: Int = TRACE_LINES,
        maxBytes: Int = TRACE_BYTES,
        scanLimit: Long = 4L * 1024 * 1024,
    ): String {
        val lines = stream.bufferedReader().lineSequence().iterator()
        val head = ArrayList<String>()
        var scanned = 0L
        while (lines.hasNext() && scanned < scanLimit) {
            val line = lines.next()
            scanned += line.length + 1
            if (line.startsWith("\"main\"")) {
                // The section ends at the first blank line (the next thread follows).
                return bounded(sequenceOf(line) + lines.asSequence().takeWhile { it.isNotBlank() }, maxLines, maxBytes)
            }
            if (head.size <= maxLines) head += line
        }
        return bounded(head.asSequence(), maxLines, maxBytes)
    }

    private fun bounded(lines: Sequence<String>, maxLines: Int, maxBytes: Int): String {
        val out = StringBuilder()
        var count = 0
        for (line in lines) {
            if (count >= maxLines || out.length + line.length + 1 > maxBytes) {
                out.append("…\n")
                break
            }
            out.append(line).append('\n')
            count++
        }
        return out.toString().trimEnd('\n')
    }

    /** Append, keeping only the newest [FILE_LIMIT] bytes. */
    fun append(file: File, text: String, limit: Int = FILE_LIMIT) {
        file.parentFile?.mkdirs()
        val existing = if (file.exists()) file.readText() else ""
        var combined = existing + text
        if (combined.length > limit) {
            combined = combined.substring(combined.length - limit)
            // Start on a whole line.
            combined = combined.substring(combined.indexOf('\n') + 1)
        }
        file.writeText(combined)
    }
}
