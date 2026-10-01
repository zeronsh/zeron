package sh.zeron.android.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.zeron_core.SubagentGroups
import uniffi.zeron_core.SubagentState
import uniffi.zeron_core.SubagentView

class SubagentsTest {
    private fun view(id: String, state: SubagentState, started: Long = 0, updated: Long = started, type: String? = null) =
        SubagentView(
            docId = "chat--sub--$id",
            spawnId = id,
            title = id,
            description = id,
            agentType = type,
            model = null,
            state = state,
            startedAtMs = started,
            updatedAtMs = updated,
            summary = null,
            spawnFailed = false,
        )

    private fun groups(active: Int = 0, completed: Int = 0, failed: Int = 0) = SubagentGroups(
        active = List(active) { view("run-$it", SubagentState.RUNNING) },
        completed = List(completed) { view("ok-$it", SubagentState.COMPLETED) },
        failed = List(failed) { view("bad-$it", SubagentState.FAILED) },
        running = active.toUInt(),
        harness = null,
        harnessLabel = null,
        modelLabel = null,
    )

    private fun shape(slots: List<SubagentSlot>): List<String> = slots.map {
        when (it) {
            is SubagentSlot.Item -> if (it.nested) "  ${it.view.spawnId}" else it.view.spawnId
            is SubagentSlot.Header -> (if (it.nested) "  " else "") + it.label + if (it.open) " v" else " >"
            is SubagentSlot.ShowMore -> "  ${it.label}"
        }
    }

    @Test fun finishedStartsClosedAndItsListsStartOpen() {
        val state = SubagentPanelState()
        assertFalse(state.isOpen(SubagentGroup.Finished))
        assertTrue(state.isOpen(SubagentGroup.Completed))
        assertTrue(state.isOpen(SubagentGroup.Failed))
    }

    @Test fun runningLeadAndFinishedFoldsIntoOneClosedGroup() {
        val g = groups(active = 2, completed = 2, failed = 1)
        assertEquals(listOf("run-0", "run-1", "Finished (3) >"), shape(Subagents.slots(g, SubagentPanelState())))
        val open = SubagentPanelState().toggled(SubagentGroup.Finished)
        assertEquals(
            listOf("run-0", "run-1", "Finished (3) v", "  Completed (2) v", "  ok-0", "  ok-1", "  Failed (1) v", "  bad-0"),
            shape(Subagents.slots(g, open)),
        )
        // A list collapses on its own; closing Finished keeps its state for next time.
        val failedClosed = open.toggled(SubagentGroup.Failed)
        assertEquals(listOf("run-0", "run-1", "Finished (3) v", "  Completed (2) v", "  ok-0", "  ok-1", "  Failed (1) >"), shape(Subagents.slots(g, failedClosed)))
        val reopened = failedClosed.toggled(SubagentGroup.Finished).toggled(SubagentGroup.Finished)
        assertEquals(shape(Subagents.slots(g, failedClosed)), shape(Subagents.slots(g, reopened)))
    }

    @Test fun emptyListsAndGroupsDrawNothing() {
        assertEquals(emptyList<String>(), shape(Subagents.slots(groups(), SubagentPanelState())))
        val open = SubagentPanelState().toggled(SubagentGroup.Finished)
        assertEquals(listOf("run-0"), shape(Subagents.slots(groups(active = 1), open)))
        assertEquals(listOf("Finished (1) v", "  Failed (1) v", "  bad-0"), shape(Subagents.slots(groups(failed = 1), open)))
    }

    @Test fun longListsPageAtTen() {
        val g = groups(completed = 25)
        var state = SubagentPanelState().toggled(SubagentGroup.Finished)
        var slots = Subagents.slots(g, state)
        assertEquals(1 + 1 + 10 + 1, slots.size)
        assertEquals(SubagentSlot.ShowMore(SubagentGroup.Completed, 10), slots.last())
        assertEquals("Show 10 more", (slots.last() as SubagentSlot.ShowMore).label)
        state = state.pagedUp(SubagentGroup.Completed)
        slots = Subagents.slots(g, state)
        assertEquals(1 + 1 + 20 + 1, slots.size)
        assertEquals(SubagentSlot.ShowMore(SubagentGroup.Completed, 5), slots.last())
        state = state.pagedUp(SubagentGroup.Completed)
        slots = Subagents.slots(g, state)
        assertEquals("all 25 shown, no Show more", 1 + 1 + 25, slots.size)
        // Paging one list leaves the other at its first page.
        assertEquals(Subagents.PAGE, state.shown(SubagentGroup.Failed))
    }

    @Test fun countsCapAtNinetyNine() {
        assertEquals("0", Subagents.countLabel(0))
        assertEquals("7", Subagents.countLabel(7))
        assertEquals("99", Subagents.countLabel(99))
        assertEquals("99+", Subagents.countLabel(100))
        assertEquals("99+", Subagents.countLabel(4_000_000_000u))
        assertEquals("12", Subagents.countLabel(12u))
    }

    @Test fun timesReadShort() {
        assertEquals("45s", Subagents.durationLabel(45_000))
        assertEquals("12m", Subagents.durationLabel(12 * 60_000L + 5_000))
        assertEquals("3h 05m", Subagents.durationLabel(3 * 3_600_000L + 5 * 60_000L))
        assertEquals("2d 4h", Subagents.durationLabel(2 * 86_400_000L + 4 * 3_600_000L))
        assertEquals("now", Subagents.agoLabel(20_000))
        assertEquals("4m ago", Subagents.agoLabel(4 * 60_000L))
        val now = 10 * 60_000L
        assertEquals("Explore · running 10m", Subagents.subtitle(view("a", SubagentState.RUNNING, type = "Explore"), now))
        assertEquals("completed 4m ago", Subagents.subtitle(view("b", SubagentState.COMPLETED, updated = 6 * 60_000L), now))
    }

    @Test fun findsAcrossGroups() {
        val g = groups(active = 1, completed = 1, failed = 1)
        assertEquals(SubagentState.FAILED, Subagents.find(g, "chat--sub--bad-0")?.state)
        assertEquals(null, Subagents.find(g, "chat--sub--nope"))
        assertEquals(3, Subagents.total(g))
    }
}
