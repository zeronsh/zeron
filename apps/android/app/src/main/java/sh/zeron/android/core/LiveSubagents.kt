package sh.zeron.android.core

/**
 * The running-subagent counts the app itself knows, from the chats it has open
 * (their subagent chips), keyed by chat id. They are merged with the counts the
 * engines publish on the session rows ([SessionActivity.merged]) so the thread
 * and the list always agree.
 *
 * A count outlives its source briefly so nothing flickers: a chat that closes
 * keeps its last count for [RELEASE_GRACE_MS] (the list the user lands on
 * shows it until the engine's own row catches up), and a count that reads
 * zero keeps the old one for [ZERO_GRACE_MS] (a transient empty read of the
 * chat's chips is not "all subagents finished"). Pure: callers pass the clock.
 */
class LiveSubagents {
    private val counts = HashMap<String, Int>()
    private val expiry = HashMap<String, Long>()

    /** The open chat [chatId] currently has [count] running subagents. Returns whether [snapshot] changed. */
    fun report(chatId: String, count: Int, now: Long): Boolean {
        val before = counts[chatId]
        if (count > 0) {
            counts[chatId] = count
            expiry.remove(chatId)
        } else if (before != null && !expiry.containsKey(chatId)) {
            expiry[chatId] = now + ZERO_GRACE_MS
        }
        return counts[chatId] != before
    }

    /** The chat was closed (or its document stopped): keep its count a little longer. */
    fun release(chatId: String, now: Long) {
        if (counts.containsKey(chatId)) expiry[chatId] = minOf(expiry[chatId] ?: Long.MAX_VALUE, now + RELEASE_GRACE_MS)
    }

    /** Drops counts whose grace has run out. Returns whether [snapshot] changed. */
    fun expire(now: Long): Boolean {
        val due = expiry.filterValues { it <= now }.keys
        due.forEach { counts.remove(it); expiry.remove(it) }
        return due.isNotEmpty()
    }

    /** When [expire] next has something to do, or null. */
    fun nextExpiry(): Long? = expiry.values.minOrNull()

    fun snapshot(): Map<String, Int> = HashMap(counts)

    companion object {
        const val RELEASE_GRACE_MS = 20_000L
        const val ZERO_GRACE_MS = 3_000L
    }
}
