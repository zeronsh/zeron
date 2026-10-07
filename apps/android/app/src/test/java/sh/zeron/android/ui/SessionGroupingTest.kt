package sh.zeron.android.ui

import org.junit.Assert.assertEquals
import org.junit.Test
import uniffi.zeron_core.ChatIndicator
import uniffi.zeron_core.ProjectRef
import uniffi.zeron_core.SessionRow

class SessionGroupingTest {
    private fun row(
        id: String,
        project: String? = null,
        indicator: ChatIndicator = ChatIndicator.IDLE,
        unseen: Boolean = false,
        at: Long = 0,
        pinned: Boolean = false,
    ) = SessionRow(
        id = id,
        revision = 0u,
        title = "Session $id",
        hasTitle = true,
        preview = null,
        project = project?.let { ProjectRef(id = it, name = it.uppercase(), colorIndex = 2u) },
        deviceId = "dev",
        deviceName = "Studio",
        deviceOnline = true,
        harness = "claude-code",
        harnessLabel = null,
        model = null,
        modelLabel = null,
        reasoning = null,
        branch = null,
        cwd = null,
        indicator = indicator,
        hostIndicator = indicator,
        lastOutcome = indicator,
        workingSinceMs = null,
        lastActivityMs = at,
        timeLabel = "now",
        createdAtMs = at,
        unseen = unseen,
        archived = false,
        pinned = pinned,
        sectionId = null,
        pullRequest = null,
        sendState = null,
        parentChatId = null,
        roomGen = 0u,
    )

    @Test
    fun groupsByProjectInFirstAppearanceOrder() {
        val rows = listOf(row("1", "zeron"), row("2", "edge"), row("3", "zeron"), row("4"), row("5", "edge"))
        val groups = SessionGrouping.projectGroups(rows)
        assertEquals(listOf("project:zeron", "project:edge", "project:~"), groups.map { it.key })
        assertEquals(listOf("ZERON", "EDGE", "~"), groups.map { it.title })
        assertEquals(listOf("1", "3"), groups[0].rows.map { it.id })
        assertEquals(listOf("2", "5"), groups[1].rows.map { it.id })
        assertEquals(2, groups[0].colorIndex)
    }

    @Test
    fun projectlessSessionsShareTheTildeGroup() {
        val groups = SessionGrouping.projectGroups(listOf(row("a"), row("b", "p"), row("c")))
        val home = groups.single { it.projectId == null }
        assertEquals(SessionGrouping.HOME_KEY, home.key)
        assertEquals("~", home.title)
        assertEquals(null, home.colorIndex)
        assertEquals(listOf("a", "c"), home.rows.map { it.id })
    }

    @Test
    fun emptyInputHasNoGroups() {
        assertEquals(emptyList<ProjectGroup>(), SessionGrouping.projectGroups(emptyList()))
    }

    @Test
    fun activityTiersLiveThenUnreadThenRest() {
        val rows = listOf(
            row("old-idle", at = 100),
            row("unread", indicator = ChatIndicator.COMPLETED, unseen = true, at = 50),
            row("working", indicator = ChatIndicator.WORKING, at = 10),
            row("input", indicator = ChatIndicator.AWAITING_INPUT, at = 20),
            row("recent-idle", at = 300),
            row("errored-seen", indicator = ChatIndicator.ERRORED, at = 200),
        )
        assertEquals(
            listOf("input", "working", "unread", "recent-idle", "errored-seen", "old-idle"),
            SessionGrouping.activityOrder(rows).map { it.id },
        )
    }

    @Test
    fun activityRecencyWithinTierAndIdTieBreak() {
        val rows = listOf(row("b", at = 5), row("a", at = 5), row("c", at = 9))
        assertEquals(listOf("c", "a", "b"), SessionGrouping.activityOrder(rows).map { it.id })
    }

    @Test
    fun activityDeduplicatesPinnedAndRecent() {
        val pinned = row("p", pinned = true, at = 1)
        val rows = listOf(pinned, row("x", at = 2), pinned)
        assertEquals(listOf("x", "p"), SessionGrouping.activityOrder(rows).map { it.id })
    }

    @Test
    fun rowProjectLabelIsTheProjectOrTilde() {
        assertEquals("ZERON", SessionGrouping.rowProjectLabel(row("a", project = "zeron", pinned = true)))
        // Project-less: "~" like the By Project group, not the host ("Studio").
        assertEquals("~", SessionGrouping.rowProjectLabel(row("b")))
    }
}
