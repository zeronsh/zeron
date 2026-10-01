package sh.zeron.android.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import sh.zeron.android.core.FavoriteModel
import sh.zeron.android.core.FavoritesCodec
import sh.zeron.android.core.NewSessionDraft
import uniffi.zeron_core.ContextUsage
import uniffi.zeron_core.RepoRef

class FavoritesTest {
    @Test fun roundTripKeepsStarringOrder() {
        val list = listOf(FavoriteModel("codex", "gpt-5"), FavoriteModel("claude-code", "opus"))
        assertEquals(list, FavoritesCodec.decode(FavoritesCodec.encode(list)))
    }

    @Test fun togglingAppendsAndRemoves() {
        val a = FavoriteModel("codex", "gpt-5")
        val b = FavoriteModel("claude-code", "opus")
        val starred = FavoritesCodec.toggled(FavoritesCodec.toggled(emptyList(), a), b)
        assertEquals(listOf(a, b), starred)
        assertEquals(listOf(b), FavoritesCodec.toggled(starred, a))
        // Re-starring goes to the end: starring order, not catalog order.
        assertEquals(listOf(b, a), FavoritesCodec.toggled(listOf(b), a))
    }

    @Test fun corruptOrPartialDataIsTolerated() {
        assertEquals(emptyList<FavoriteModel>(), FavoritesCodec.decode(null))
        assertEquals(emptyList<FavoriteModel>(), FavoritesCodec.decode("not json"))
        assertEquals(
            listOf(FavoriteModel("codex", "gpt-5")),
            FavoritesCodec.decode("""[{"harness":"codex","model":"gpt-5"},{"harness":"codex"},42,{"harness":"codex","model":"gpt-5"}]"""),
        )
    }
}

class BranchRulesTest {
    private val refs = listOf(
        RepoRef("main", true, null),
        RepoRef("feature/diff-pane", false, null),
        RepoRef("veil-fade", false, "/home/me/.zeron/worktrees/zeron-veil-fade"),
    )

    @Test fun filterMatchesNamesCaseInsensitively() {
        assertEquals(listOf("feature/diff-pane"), BranchRules.filter(refs, " DIFF ").map { it.name })
        assertEquals(refs, BranchRules.filter(refs, ""))
    }

    @Test fun pickFollowsTheDesktopRules() {
        val draft = NewSessionDraft(projectId = "p")
        // An existing worktree is reused as the session's folder.
        assertEquals(
            RefPick.Set(draft.copy(branch = "veil-fade", cwd = "/home/me/.zeron/worktrees/zeron-veil-fade")),
            BranchRules.pick(draft.copy(worktree = true), refs[2]),
        )
        // The checked-out branch is just recorded.
        assertEquals(RefPick.Set(draft.copy(branch = "main")), BranchRules.pick(draft, refs[0]))
        // Another branch checks the project folder out…
        assertEquals(RefPick.Checkout("feature/diff-pane"), BranchRules.pick(draft, refs[1]))
        // …unless it's the base of a new worktree.
        assertEquals(RefPick.Set(draft.copy(worktree = true, branch = "feature/diff-pane")), BranchRules.pick(draft.copy(worktree = true), refs[1]))
    }

    @Test fun leavingWorktreeModeDropsABaseTheFolderIsNotOn() {
        val base = NewSessionDraft(projectId = "p", worktree = true, branch = "feature/diff-pane")
        assertEquals(base.copy(worktree = false, branch = null), BranchRules.checkout(base, false, refs))
        val onMain = base.copy(branch = "main")
        assertEquals(onMain.copy(worktree = false), BranchRules.checkout(onMain, false, refs))
        assertTrue(BranchRules.checkout(NewSessionDraft(cwd = "/x"), true, refs).let { it.worktree && it.cwd == null })
    }

    @Test fun chipNamesTheCheckedOutBranch() {
        assertEquals("main", BranchRules.label(NewSessionDraft(), refs))
        assertEquals("Branch", BranchRules.label(NewSessionDraft(), null))
        assertEquals("New worktree · main", BranchRules.label(NewSessionDraft(worktree = true), refs))
        assertEquals("veil-fade", BranchRules.label(NewSessionDraft(branch = "veil-fade"), refs))
        assertEquals("main", BranchRules.selected(refs, NewSessionDraft()))
    }
}

class ContextUsageTextTest {
    @Test fun indicatorNeedsAReportedWindow() {
        assertFalse(ContextUsageText.hasWindow(null))
        assertFalse(ContextUsageText.hasWindow(ContextUsage(1_200uL, null)))
        assertFalse(ContextUsageText.hasWindow(ContextUsage(1_200uL, 0uL)))
        assertTrue(ContextUsageText.hasWindow(ContextUsage(null, 200_000uL)))
    }

    @Test fun detailsMatchTheDesktopCard() {
        assertEquals(listOf("5,417 / 1,048,576 tokens", "1,043,159 tokens remaining"), ContextUsageText.details(ContextUsage(5417uL, 1_048_576uL)))
        assertEquals("0 tokens remaining", ContextUsageText.details(ContextUsage(250uL, 200uL))[1])
        assertEquals(listOf("10 tokens used", "Context limit not reported"), ContextUsageText.details(ContextUsage(10uL, 0uL)))
        assertEquals("Waiting for context usage", ContextUsageText.details(ContextUsage(null, 200uL))[1])
        assertTrue(ContextUsageText.details(null).single().contains("not reported"))
    }

    @Test fun percentAndLevels() {
        assertEquals("42%", ContextUsageText.percent(ContextUsage(42uL, 100uL)))
        assertEquals("—", ContextUsageText.percent(ContextUsage(null, 100uL)))
        assertNull(ContextUsageText.fraction(ContextUsage(5uL, 0uL)))
        assertEquals(ContextUsageText.Level.Normal, ContextUsageText.level(ContextUsage(74uL, 100uL)))
        assertEquals(ContextUsageText.Level.Warning, ContextUsageText.level(ContextUsage(75uL, 100uL)))
        assertEquals(ContextUsageText.Level.Danger, ContextUsageText.level(ContextUsage(90uL, 100uL)))
        assertEquals("1,000", ContextUsageText.grouped(1000uL))
        assertEquals("999", ContextUsageText.grouped(999uL))
    }
}
