package sh.zeron.android.ui

import uniffi.zeron_core.ChatIndicator
import uniffi.zeron_core.SessionRow

/** One "By Project" group: a project's sessions, or the project-less "~" group. */
data class ProjectGroup(
    /** Collapse id: `project:<id>`, or `project:~` for project-less sessions. */
    val key: String,
    val projectId: String?,
    val title: String,
    val colorIndex: Int?,
    val rows: List<SessionRow>,
)

/**
 * Home list grouping and ordering (pure, unit-tested). The incoming lists are
 * already in the core's iOS sort order; these functions only partition or
 * re-rank, never re-sort within a group.
 */
object SessionGrouping {
    /** Desktop sidebar label for sessions without a project. */
    const val HOME_LABEL = "~"
    const val HOME_KEY = "project:~"

    fun projectKey(projectId: String?): String = if (projectId == null) HOME_KEY else "project:$projectId"

    /**
     * The project name on a row's second line (Pinned group, By Activity, and
     * inside groups): the project, or "~" for project-less sessions, the
     * same label as their By Project group, never the host's name.
     */
    fun rowProjectLabel(row: SessionRow): String =
        row.project?.name?.takeIf { it.isNotBlank() } ?: if (row.project == null) HOME_LABEL else "?"

    /**
     * One group per project, in order of each project's first session (so the
     * group holding the most recent session comes first); rows keep their
     * incoming order.
     */
    fun projectGroups(rows: List<SessionRow>): List<ProjectGroup> {
        val groups = LinkedHashMap<String, MutableList<SessionRow>>()
        for (row in rows) groups.getOrPut(projectKey(row.project?.id)) { ArrayList() }.add(row)
        return groups.map { (key, list) ->
            val project = list.first().project
            ProjectGroup(
                key = key,
                projectId = project?.id,
                title = project?.name?.takeIf { it.isNotBlank() } ?: if (project == null) HOME_LABEL else "?",
                colorIndex = project?.colorIndex?.toInt(),
                rows = list,
            )
        }
    }

    /**
     * Activity tier: 0 live turns (working or waiting for input), 1 finished
     * but unread, 2 everything else. Unknown indicators fall through by
     * `unseen`.
     */
    fun activityTier(row: SessionRow): Int = when {
        row.indicator == ChatIndicator.WORKING || row.indicator == ChatIndicator.AWAITING_INPUT -> 0
        row.unseen -> 1
        else -> 2
    }

    /**
     * "By Activity": every session once (first occurrence wins), live turns
     * first, then unread, then the rest; most recent first within a tier.
     */
    fun activityOrder(rows: List<SessionRow>): List<SessionRow> {
        val seen = HashSet<String>()
        return rows.filter { seen.add(it.id) }.sortedWith(
            compareBy<SessionRow> { activityTier(it) }
                .thenByDescending { it.lastActivityMs }
                .thenBy { it.id },
        )
    }
}
