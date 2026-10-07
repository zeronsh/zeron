package sh.zeron.android.schedule

import android.app.AlarmManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import org.json.JSONArray
import org.json.JSONObject
import java.util.Calendar
import java.util.TimeZone
import java.util.UUID

/**
 * What a scheduled first message starts: the New Session screen's picks at
 * the moment it was scheduled (project or projectless host, CLI, model,
 * effort, branch / new worktree). [label] (project or host name) is for
 * display only.
 */
data class NewSessionSpec(
    val projectId: String? = null,
    val hostId: String? = null,
    val harness: String,
    val model: String? = null,
    val effort: String? = null,
    val branch: String? = null,
    val worktree: Boolean = false,
    val projectPath: String? = null,
    val label: String = "",
) {
    fun toJson(): JSONObject = JSONObject()
        .put("projectId", projectId ?: JSONObject.NULL).put("hostId", hostId ?: JSONObject.NULL)
        .put("harness", harness).put("model", model ?: JSONObject.NULL).put("effort", effort ?: JSONObject.NULL)
        .put("branch", branch ?: JSONObject.NULL).put("worktree", worktree)
        .put("projectPath", projectPath ?: JSONObject.NULL).put("label", label)

    companion object {
        private fun JSONObject.str(key: String): String? = if (isNull(key)) null else optString(key).ifBlank { null }

        fun fromJson(o: JSONObject) = NewSessionSpec(
            projectId = o.str("projectId"),
            hostId = o.str("hostId"),
            harness = o.getString("harness"),
            model = o.str("model"),
            effort = o.str("effort"),
            branch = o.str("branch"),
            worktree = o.optBoolean("worktree"),
            projectPath = o.str("projectPath"),
            label = o.optString("label"),
        )
    }
}

/**
 * A message to send later. [atMs] is phone wall-clock epoch millis. Either
 * into an existing session ([chatId]) or, with [newSession], as the first
 * message of a session created at fire time ([chatId] is then empty).
 */
data class ScheduledMessage(
    val id: String = UUID.randomUUID().toString(),
    /** `demo`, `cloud` or a saved machine id (see CoreConnect). */
    val workspace: String,
    val chatId: String,
    val text: String,
    val atMs: Long,
    /** For notifications only. */
    val chatTitle: String = "",
    val newSession: NewSessionSpec? = null,
) {
    fun toJson(): JSONObject = JSONObject()
        .put("id", id).put("workspace", workspace).put("chatId", chatId)
        .put("text", text).put("atMs", atMs).put("chatTitle", chatTitle)
        .apply { newSession?.let { put("newSession", it.toJson()) } }

    companion object {
        fun fromJson(o: JSONObject) = ScheduledMessage(
            id = o.getString("id"),
            workspace = o.getString("workspace"),
            chatId = o.getString("chatId"),
            text = o.getString("text"),
            atMs = o.getLong("atMs"),
            chatTitle = o.optString("chatTitle"),
            newSession = o.optJSONObject("newSession")?.let { NewSessionSpec.fromJson(it) },
        )
    }
}

/** Pure time math for the picker (unit-tested). Uses the phone's clock and zone only. */
object ScheduleTime {
    /** The next [hour]:[minute] from [nowMs]: today, or tomorrow when that has passed. */
    fun next(hour: Int, minute: Int, nowMs: Long, zone: TimeZone = TimeZone.getDefault()): Long {
        val cal = Calendar.getInstance(zone).apply {
            timeInMillis = nowMs
            set(Calendar.HOUR_OF_DAY, hour)
            set(Calendar.MINUTE, minute)
            set(Calendar.SECOND, 0)
            set(Calendar.MILLISECOND, 0)
        }
        if (cal.timeInMillis <= nowMs) cal.add(Calendar.DAY_OF_MONTH, 1)
        return cal.timeInMillis
    }

    fun isSameDay(aMs: Long, bMs: Long, zone: TimeZone = TimeZone.getDefault()): Boolean {
        val a = Calendar.getInstance(zone).apply { timeInMillis = aMs }
        val b = Calendar.getInstance(zone).apply { timeInMillis = bMs }
        return a.get(Calendar.YEAR) == b.get(Calendar.YEAR) && a.get(Calendar.DAY_OF_YEAR) == b.get(Calendar.DAY_OF_YEAR)
    }

    /** "After" mode: [minutes] from [nowMs], to the millisecond (the alarm is exact when allowed). */
    fun after(minutes: Int, nowMs: Long): Long = nowMs + minutes.coerceAtLeast(0) * 60_000L

    /** Calendar days from [nowMs]'s date to [atMs]'s date: 0 today, 1 tomorrow… */
    fun daysFrom(atMs: Long, nowMs: Long, zone: TimeZone = TimeZone.getDefault()): Int {
        fun dayStart(ms: Long) = Calendar.getInstance(zone).apply {
            timeInMillis = ms
            set(Calendar.HOUR_OF_DAY, 12) // noon: DST-safe day arithmetic
            set(Calendar.MINUTE, 0)
            set(Calendar.SECOND, 0)
            set(Calendar.MILLISECOND, 0)
        }.timeInMillis
        return Math.round((dayStart(atMs) - dayStart(nowMs)) / 86_400_000.0).toInt()
    }

    /** "01:20" in 24-hour time (the chip's format). */
    fun clock(ms: Long, zone: TimeZone = TimeZone.getDefault()): String {
        val c = Calendar.getInstance(zone).apply { timeInMillis = ms }
        return String.format(java.util.Locale.US, "%02d:%02d", c.get(Calendar.HOUR_OF_DAY), c.get(Calendar.MINUTE))
    }
}

/** Scheduled messages, persisted in private prefs; [changes] ticks on every write. */
class ScheduledStore(context: Context) {
    private val prefs = context.applicationContext.getSharedPreferences("zeron-scheduled", 0)

    fun list(): List<ScheduledMessage> {
        val raw = prefs.getString("messages", null) ?: return emptyList()
        return runCatching {
            val arr = JSONArray(raw)
            (0 until arr.length()).map { ScheduledMessage.fromJson(arr.getJSONObject(it)) }
        }.getOrDefault(emptyList()).sortedBy { it.atMs }
    }

    fun get(id: String): ScheduledMessage? = list().firstOrNull { it.id == id }

    fun add(message: ScheduledMessage) = synchronized(LOCK) { write(list().filter { it.id != message.id } + message) }

    /** Removes and returns the message, or null when it was already gone (claimed). */
    fun take(id: String): ScheduledMessage? = synchronized(LOCK) {
        val all = list()
        val hit = all.firstOrNull { it.id == id } ?: return null
        write(all.filter { it.id != id })
        hit
    }

    private fun write(all: List<ScheduledMessage>) {
        val arr = JSONArray()
        all.forEach { arr.put(it.toJson()) }
        // commit(): the alarm may fire in a fresh process right after this.
        prefs.edit().putString("messages", arr.toString()).commit()
        version.value = version.value + 1
    }

    companion object {
        private val LOCK = Any()
        private val version = MutableStateFlow(0)
        val changes: StateFlow<Int> get() = version
    }
}

/** AlarmManager side: exact when allowed, otherwise inexact (still fires while idle). */
object ScheduledAlarms {
    const val ACTION_FIRE = "sh.zeron.android.action.SCHEDULED_SEND"
    const val EXTRA_ID = "id"

    fun canExact(context: Context): Boolean {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) return true
        return context.getSystemService(AlarmManager::class.java).canScheduleExactAlarms()
    }

    /** Stores [message] and sets its alarm. Returns true when the alarm is exact. */
    fun schedule(context: Context, message: ScheduledMessage): Boolean {
        ScheduledStore(context).add(message)
        return arm(context, message)
    }

    fun cancel(context: Context, id: String) {
        ScheduledStore(context).take(id)
        context.getSystemService(AlarmManager::class.java).cancel(pending(context, id))
    }

    /** After boot, an app update or a permission change: set every stored alarm again. */
    fun restoreAll(context: Context) {
        ScheduledStore(context).list().forEach { arm(context, it) }
    }

    private fun arm(context: Context, message: ScheduledMessage): Boolean {
        val am = context.getSystemService(AlarmManager::class.java)
        // Missed while the phone was off: send a few seconds from now.
        val at = maxOf(message.atMs, System.currentTimeMillis() + 3_000)
        val pi = pending(context, message.id)
        if (canExact(context)) {
            try {
                am.setExactAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, at, pi)
                return true
            } catch (_: SecurityException) {
                // Permission revoked between the check and the call.
            }
        }
        am.setAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, at, pi)
        return false
    }

    private fun pending(context: Context, id: String): PendingIntent {
        val intent = Intent(context, ScheduledSendReceiver::class.java)
            .setAction(ACTION_FIRE)
            .setData(Uri.parse("zeron-scheduled:$id"))
            .putExtra(EXTRA_ID, id)
        return PendingIntent.getBroadcast(context, 0, intent, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
    }
}
