package sh.zeron.android.ui

import uniffi.zeron_core.SubagentGroups
import uniffi.zeron_core.SubagentState
import uniffi.zeron_core.SubagentView

/**
 * The Subagents panel's view state and shape, kept free of Compose so it is
 * testable on the JVM. Order and grouping come from Rust
 * (`zeron_client::subagents`, the desktop's rules); this mirrors the
 * desktop's `GroupState` and slot plan (crates/ui/src/files/subagents.rs):
 * running rows on top, then a **Finished (N)** dropdown — closed until opened
 * — holding **Completed (n)** and **Failed (n)** lists that start open and
 * page at ten rows. An empty list draws nothing.
 */
object Subagents {
    /** Rows a finished list shows first, and how many "Show more" adds. */
    const val PAGE = 10

    /** Past this a running count reads "99+" (the desktop pill's cap). */
    const val COUNT_CAP = 99

    fun countLabel(count: Int): String = if (count > COUNT_CAP) "$COUNT_CAP+" else "${count.coerceAtLeast(0)}"

    fun countLabel(count: UInt): String = countLabel(if (count > Int.MAX_VALUE.toUInt()) Int.MAX_VALUE else count.toInt())

    fun total(groups: SubagentGroups): Int = groups.active.size + finished(groups)

    fun finished(groups: SubagentGroups): Int = groups.completed.size + groups.failed.size

    /** "45s", "12m", "3h 05m", "2d 4h" — a running subagent's age. */
    fun durationLabel(ms: Long): String {
        val s = (ms / 1000).coerceAtLeast(0)
        return when {
            s < 60 -> "${s}s"
            s < 3600 -> "${s / 60}m"
            s < 86_400 -> "${s / 3600}h ${"%02d".format((s % 3600) / 60)}m"
            else -> "${s / 86_400}d ${(s % 86_400) / 3600}h"
        }
    }

    /** "now", "4m ago", "3h ago", "2d ago" — when a finished one was last updated. */
    fun agoLabel(ms: Long): String {
        val m = (ms / 60_000).coerceAtLeast(0)
        return when {
            m < 1 -> "now"
            m < 60 -> "${m}m ago"
            m < 1440 -> "${m / 60}h ago"
            else -> "${m / 1440}d ago"
        }
    }

    /** One line under a subagent's title: its type, then how long it has run or when it ended. */
    fun subtitle(view: SubagentView, nowMs: Long): String {
        val kind = view.agentType?.takeIf { it.isNotBlank() }
        val time = when (view.state) {
            SubagentState.RUNNING -> "running ${durationLabel(nowMs - view.startedAtMs)}"
            SubagentState.PENDING -> "starting"
            SubagentState.COMPLETED -> "completed ${agoLabel(nowMs - view.updatedAtMs)}"
            SubagentState.FAILED -> "failed ${agoLabel(nowMs - view.updatedAtMs)}"
        }
        return listOfNotNull(kind, view.model, time).joinToString(" · ")
    }

    fun stateLabel(state: SubagentState): String = when (state) {
        SubagentState.RUNNING -> "Running"
        SubagentState.PENDING -> "Starting"
        SubagentState.COMPLETED -> "Completed"
        SubagentState.FAILED -> "Failed"
    }

    fun find(groups: SubagentGroups, docId: String): SubagentView? =
        (groups.active + groups.completed + groups.failed).firstOrNull { it.docId == docId }

    /** The rows the panel draws, in order, for the groups as they stand. */
    fun slots(groups: SubagentGroups, state: SubagentPanelState): List<SubagentSlot> {
        val out = ArrayList<SubagentSlot>()
        groups.active.forEach { out += SubagentSlot.Item(it, nested = false) }
        val finished = finished(groups)
        if (finished == 0) return out
        val open = state.isOpen(SubagentGroup.Finished)
        out += SubagentSlot.Header(SubagentGroup.Finished, finished, open, nested = false)
        if (!open) return out
        for ((group, rows) in listOf(SubagentGroup.Completed to groups.completed, SubagentGroup.Failed to groups.failed)) {
            if (rows.isEmpty()) continue
            val listOpen = state.isOpen(group)
            out += SubagentSlot.Header(group, rows.size, listOpen, nested = true)
            if (!listOpen) continue
            val shown = state.shown(group)
            rows.take(shown).forEach { out += SubagentSlot.Item(it, nested = true) }
            if (rows.size > shown) out += SubagentSlot.ShowMore(group, minOf(rows.size - shown, PAGE))
        }
        return out
    }
}

/** A collapsible group of finished subagents. */
enum class SubagentGroup(val label: String) {
    /** Parent of the two below. */
    Finished("Finished"),
    Completed("Completed"),
    Failed("Failed"),
}

/**
 * Open/closed and paging state of the groups. Finished starts closed, so the
 * panel opens on what is running; the two lists inside it start open, so one
 * tap shows every finished subagent. Closing Finished keeps each list's own
 * state for the next open.
 */
data class SubagentPanelState(
    private val flipped: Set<SubagentGroup> = emptySet(),
    private val revealed: Map<SubagentGroup, Int> = emptyMap(),
) {
    fun isOpen(group: SubagentGroup): Boolean = (group != SubagentGroup.Finished) != (group in flipped)

    fun toggled(group: SubagentGroup): SubagentPanelState =
        copy(flipped = if (group in flipped) flipped - group else flipped + group)

    fun shown(group: SubagentGroup): Int = maxOf(revealed[group] ?: Subagents.PAGE, Subagents.PAGE)

    fun pagedUp(group: SubagentGroup): SubagentPanelState = copy(revealed = revealed + (group to shown(group) + Subagents.PAGE))
}

/** One row of the panel. */
sealed interface SubagentSlot {
    val key: String

    data class Item(val view: SubagentView, val nested: Boolean) : SubagentSlot {
        override val key get() = "row-${view.docId}"
    }

    data class Header(val group: SubagentGroup, val count: Int, val open: Boolean, val nested: Boolean) : SubagentSlot {
        override val key get() = "group-${group.name}"
        val label get() = "${group.label} ($count)"
    }

    /** "Show N more" — N is at most a page. */
    data class ShowMore(val group: SubagentGroup, val more: Int) : SubagentSlot {
        override val key get() = "more-${group.name}"
        val label get() = "Show $more more"
    }
}
