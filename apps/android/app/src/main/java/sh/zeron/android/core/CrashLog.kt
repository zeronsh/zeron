package sh.zeron.android.core

import android.content.Context
import android.os.Build
import org.json.JSONArray
import sh.zeron.android.BuildConfig
import java.io.File
import java.io.PrintWriter
import java.io.StringWriter
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import java.util.TimeZone

/**
 * Local-only crash log. An uncaught exception is written to app-private
 * storage (filesDir/crashes, the last [KEEP]) before the previous handler
 * (the system's "app stopped") runs; the next launch offers it in a dialog
 * and Settings > About > Crash logs lists them. Nothing is ever uploaded:
 * the user copies or shares a log by hand.
 *
 * Reports are scrubbed of IP addresses, host / computer names and user names
 * ([redact]). Release builds are minified: a report names the build, whose
 * R8 mapping (kept with the signing key) retraces the stack.
 */
object CrashLog {
    const val KEEP = 5

    /** Longest stack trace kept, so sharing stays well under the Binder limit. */
    const val MAX_TRACE_CHARS = 48_000

    private const val PENDING = "pending"

    /** Extra names to scrub (computer names from the workspace), set by the model. */
    @Volatile
    var extraNames: () -> Collection<String> = { emptyList() }

    data class Entry(val file: File, val atMs: Long, val text: String) {
        /** `java.lang.IllegalStateException: message`, the report's first exception line. */
        val headline: String
            get() = text.lineSequence().dropWhile { !it.startsWith(TRACE_MARK) }.drop(1).firstOrNull { it.isNotBlank() }?.trim().orEmpty()
    }

    internal const val TRACE_MARK = "--- stack trace ---"
    private const val THREAD = "Thread: "

    fun dir(context: Context): File = File(context.filesDir, "crashes")

    /** Chains to the handler already installed (the platform's, which shows "app stopped" and kills the process). */
    fun install(context: Context) {
        val app = context.applicationContext
        val previous = Thread.getDefaultUncaughtExceptionHandler()
        if (previous is Handler) return
        Thread.setDefaultUncaughtExceptionHandler(Handler(app, previous))
    }

    class Handler(private val context: Context, val previous: Thread.UncaughtExceptionHandler?) : Thread.UncaughtExceptionHandler {
        override fun uncaughtException(thread: Thread, error: Throwable) {
            try {
                write(context, thread.name, error)
            } catch (_: Throwable) {
                // Never let the log get in the way of the crash itself.
            }
            previous?.uncaughtException(thread, error)
        }
    }

    fun write(context: Context, threadName: String, error: Throwable, now: Long = System.currentTimeMillis()): File {
        val names = sensitiveNames(context)
        val text = report(threadName, error, now, scrub = { redact(it, names) })
        val dir = dir(context).apply { mkdirs() }
        var file = File(dir, "crash-$now.txt")
        var n = 1
        while (file.exists()) file = File(dir, "crash-$now-${n++}.txt")
        file.writeText(text)
        File(dir, PENDING).writeText(file.name)
        prune(dir)
        return file
    }

    /** The plain-text report: when, which build, which phone, which thread, the stack. */
    fun report(
        threadName: String,
        error: Throwable,
        now: Long,
        versionName: String = BuildConfig.VERSION_NAME,
        versionCode: Int = BuildConfig.VERSION_CODE,
        android: String = "${Build.VERSION.RELEASE} (API ${Build.VERSION.SDK_INT})",
        device: String = listOf(Build.MANUFACTURER, Build.MODEL).filter { !it.isNullOrBlank() }.distinct().joinToString(" "),
        scrub: (String) -> String = { it },
    ): String {
        val trace = StringWriter().also { error.printStackTrace(PrintWriter(it)) }.toString().let {
            if (it.length > MAX_TRACE_CHARS) it.take(MAX_TRACE_CHARS) + "\n\t… (truncated)\n" else it
        }
        val time = SimpleDateFormat("yyyy-MM-dd HH:mm:ss Z", Locale.ROOT).apply { timeZone = TimeZone.getDefault() }.format(Date(now))
        return buildString {
            appendLine("Zeron crash log")
            appendLine("Time: $time")
            appendLine("App: $versionName (versionCode $versionCode)")
            appendLine("Android: $android")
            appendLine("Device: $device")
            appendLine("$THREAD${scrub(threadName)}")
            appendLine(TRACE_MARK)
            append(scrub(trace.trimEnd())).append('\n')
        }
    }

    /** Saved logs, newest first (scrubbed again with today's names). */
    fun list(context: Context): List<Entry> {
        val names = sensitiveNames(context)
        return (dir(context).listFiles { f -> f.name.startsWith("crash-") && f.name.endsWith(".txt") } ?: emptyArray())
            .sortedByDescending { atOf(it) }
            .mapNotNull { f -> runCatching { Entry(f, atOf(f), redactBody(f.readText(), names)) }.getOrNull() }
    }

    /** The crash the user hasn't been told about yet (the next launch's dialog). */
    fun pending(context: Context): Entry? {
        val marker = File(dir(context), PENDING)
        val name = runCatching { marker.readText().trim() }.getOrNull() ?: return null
        return list(context).firstOrNull { it.file.name == name } ?: run { marker.delete(); null }
    }

    fun markSeen(context: Context) {
        File(dir(context), PENDING).delete()
    }

    fun clear(context: Context) {
        dir(context).listFiles()?.forEach { it.delete() }
    }

    private fun atOf(f: File): Long = Regex("""crash-(\d+)""").find(f.name)?.groupValues?.get(1)?.toLongOrNull() ?: f.lastModified()

    private fun prune(dir: File) {
        (dir.listFiles { f -> f.name.startsWith("crash-") } ?: return)
            .sortedByDescending { atOf(it) }
            .drop(KEEP)
            .forEach { it.delete() }
    }

    /**
     * Names worth scrubbing that no pattern would catch: saved computers'
     * names, hosts and SSH users, this phone's own name, and whatever the
     * model adds (workspace computer names).
     */
    fun sensitiveNames(context: Context): Set<String> {
        val out = HashSet<String>()
        runCatching {
            val raw = context.getSharedPreferences("zeron-machines", 0).getString("machines", null)
            if (raw != null) {
                val arr = JSONArray(raw)
                for (i in 0 until arr.length()) {
                    val o = arr.getJSONObject(i)
                    out += o.optString("name"); out += o.optString("host"); out += o.optString("user")
                    val eps = o.optJSONArray("endpoints")
                    if (eps != null) for (j in 0 until eps.length()) out += eps.getJSONObject(j).optString("host")
                }
            }
        }
        runCatching { android.provider.Settings.Global.getString(context.contentResolver, "device_name") }.getOrNull()?.let { out += it }
        runCatching { extraNames() }.getOrNull()?.let { out += it }
        return out.map { it.trim() }.filter { it.length >= 3 }.toSet()
    }

    private val urlHost = Regex("""\b([a-zA-Z][a-zA-Z0-9+.-]*://)(?:[^/\s@]+@)?(\[[^\]\s]*\]|[^/\s:?#]+)""")
    private val userAtHost = Regex("""(?<![\w.$/])[A-Za-z0-9._%+-]+@(?=[A-Za-z0-9-]*[g-zG-Z])[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*""")
    private val homePath = Regex("""(/(?:home|Users)/|[A-Za-z]:\\+Users\\+)[^/\\\s:]+""")
    private val ipv4 = Regex("""(?<![\w.])(?:\d{1,3}\.){3}\d{1,3}(?!\w|\.\d)""")
    private val ipv6 = Regex("""(?<![\w:.])[0-9A-Fa-f:]*:[0-9A-Fa-f:.]*(?:%[\w.-]+)?(?![\w:])""")
    private val localHost = Regex("""(?<![\w.-])[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.(?:local|lan|localdomain|internal|home\.arpa|ts\.net|tailnet)\b(?!\.?[\w$])""")

    /** [redact] from the Thread line on: the header's build and phone model stay. */
    internal fun redactBody(text: String, names: Collection<String>): String {
        val at = text.indexOf("\n$THREAD")
        return if (at < 0) redact(text, names) else text.substring(0, at + 1) + redact(text.substring(at + 1), names)
    }

    /** Words of package / thread names that a computer happens to share; never scrubbed. */
    private val keepWords = setOf("android", "zeron", "java", "javax", "kotlin", "kotlinx", "main", "uniffi", "jna", "sun", "com", "org", "app", "core", "androidx", "dalvik", "system")

    /**
     * Scrubs [text]: URL hosts, `user@host` (and e-mail addresses), home
     * directories, IPv4 / IPv6 addresses, `.local` / `.lan` / Tailscale host
     * names, and every name in [names] (case-insensitive). Class names,
     * `Object@1a2b3c` hashes and `File.kt:42` positions survive.
     */
    fun redact(text: String, names: Collection<String> = emptyList()): String {
        var s = text
        for (name in names.map { it.trim() }.filter { it.length >= 3 && it.lowercase(Locale.ROOT) !in keepWords }.sortedByDescending { it.length }) {
            s = Regex("""(?<![\w])""" + Regex.escape(name) + """(?![\w])""", RegexOption.IGNORE_CASE).replace(s, "<name>")
        }
        s = urlHost.replace(s) { it.groupValues[1] + "<host>" }
        s = userAtHost.replace(s, "<user>@<host>")
        s = homePath.replace(s) { it.groupValues[1] + "<user>" }
        s = ipv4.replace(s, "<ip>")
        s = ipv6.replace(s) { m ->
            val v = m.value
            val colons = v.count { it == ':' }
            val hex = v.count { it.isLetterOrDigit() }
            if ((v.contains("::") && hex >= 1) || (colons >= 3 && hex >= 4)) "<ip>" else v
        }
        s = localHost.replace(s, "<host>")
        return s
    }
}
