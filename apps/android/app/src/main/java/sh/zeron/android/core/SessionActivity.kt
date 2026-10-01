package sh.zeron.android.core

import uniffi.zeron_core.ChatIndicator
import uniffi.zeron_core.SessionRow

object SessionActivity {
    enum class Shape(val description: String) {
        MainRunning("Main agent running"),
        SubagentsRunning("Subagents running; main agent idle"),
        CallbackWaiting("Waiting for a background callback"),
    }

    fun shape(indicator: ChatIndicator, subagents: UInt, callbacks: UInt): Shape? = when (indicator) {
        ChatIndicator.WORKING -> Shape.MainRunning
        ChatIndicator.AWAITING_INPUT, ChatIndicator.ERRORED -> null
        else -> when {
            subagents > 0u -> Shape.SubagentsRunning
            callbacks > 0u -> Shape.CallbackWaiting
            else -> null
        }
    }

    fun isWorking(indicator: ChatIndicator, subagents: UInt, callbacks: UInt): Boolean =
        indicator == ChatIndicator.WORKING || subagents > 0u || callbacks > 0u

    fun isWorking(row: SessionRow): Boolean = isWorking(row.indicator, row.runningSubagents, row.pendingCallbacks)

    /**
     * The one rule for a chat's running-subagent count. Two sources know it: the
     * count the hosting engine publishes on the chat's status row ([published]:
     * absent from older engines, zeroed once the row is 45 s stale, only counted
     * once a subagent has streamed), and the subagent chips of a chat this app has
     * open ([live]). The badge shows the larger, so a list row can never say less
     * than the thread the user is looking at, nor the thread less than its row.
     */
    fun mergedSubagents(published: UInt, live: Int?): UInt =
        maxOf(published, (live ?: 0).coerceIn(0, Int.MAX_VALUE).toUInt())

    /** [row] with its subagent count merged with what the open chat shows (see [mergedSubagents]). */
    fun merged(row: SessionRow, live: Map<String, Int>): SessionRow {
        val count = mergedSubagents(row.runningSubagents, live[row.id])
        return if (count == row.runningSubagents) row else row.copy(runningSubagents = count)
    }

    fun merged(rows: List<SessionRow>, live: Map<String, Int>): List<SessionRow> =
        if (live.isEmpty()) rows else rows.map { merged(it, live) }

    /** Overflow uses an icon; neither it nor the hidden zero badge has text. */
    fun badgeLabel(count: UInt): String? = when {
        count == 0u || count > 99u -> null
        else -> count.toString()
    }
}
