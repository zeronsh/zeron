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
import uniffi.zeron_core.ModelInfo
import uniffi.zeron_core.RepoRef

private fun m(harness: String, id: String, label: String = id) =
    ModelChoice(harness, harness.uppercase(), ModelInfo(id, label, null, emptyList(), emptyList(), null))

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

class ModelPickerRulesTest {
    private val catalog = listOf(m("claude-code", "opus"), m("claude-code", "sonnet"), m("codex", "gpt-5"), m("opencode", "glm"))
    private val providers = ModelPickerRules.providers(catalog, "claude-code", locked = false) { it }

    @Test fun railListsOfferedProvidersInCatalogOrder() {
        assertEquals(listOf("claude-code", "codex", "opencode"), providers.map { it.harness })
        // A session keeps its harness: only its own provider, even before its catalog loads.
        assertEquals(listOf("codex"), ModelPickerRules.providers(catalog, "codex", locked = true) { it }.map { it.harness })
        assertEquals(listOf(PickerProvider("pi", "Pi")), ModelPickerRules.providers(catalog, "pi", locked = true) { "Pi" })
    }

    @Test fun railOpensOnFavoritesOnlyWhenTheCurrentModelIsStarred() {
        val codex = FavoriteModel("codex", "gpt-5")
        val opus = FavoriteModel("claude-code", "opus")
        assertEquals(ModelRail.Favorites, ModelPickerRules.defaultRail(listOf(codex, opus), opus, false, providers))
        assertEquals(ModelRail.Provider("claude-code"), ModelPickerRules.defaultRail(listOf(codex), opus, false, providers))
        assertEquals(ModelRail.Provider("claude-code"), ModelPickerRules.defaultRail(emptyList(), opus, false, providers))
        // Locked sessions stay on their harness.
        assertEquals(ModelRail.Provider("claude-code"), ModelPickerRules.defaultRail(listOf(opus), opus, true, providers))
        // A current harness the device doesn't offer falls back to the first provider.
        assertEquals(ModelRail.Provider("claude-code"), ModelPickerRules.defaultRail(emptyList(), FavoriteModel("grok", "x"), false, providers))
        assertEquals(ModelRail.Favorites, ModelPickerRules.defaultRail(emptyList(), null, false, emptyList()))
    }

    @Test fun favoritesViewKeepsStarringOrderAndDropsUnofferedModels() {
        val favorites = listOf(FavoriteModel("codex", "gpt-5"), FavoriteModel("grok", "x"), FavoriteModel("claude-code", "opus"), FavoriteModel("codex", "gone"))
        val rows = ModelPickerRules.rows(ModelRail.Favorites, catalog, favorites, providers, null)
        assertEquals(listOf("gpt-5", "opus"), rows.map { it.model.id })
    }

    @Test fun providerViewKeepsCatalogOrderAndShowsAnUnlistedSelection() {
        val rows = ModelPickerRules.rows(ModelRail.Provider("claude-code"), catalog, listOf(FavoriteModel("claude-code", "sonnet")), providers, null)
        assertEquals(listOf("opus", "sonnet"), rows.map { it.model.id })
        val custom = m("claude-code", "opus-preview")
        assertEquals(
            listOf("opus-preview", "opus", "sonnet"),
            ModelPickerRules.rows(ModelRail.Provider("claude-code"), catalog, emptyList(), providers, custom).map { it.model.id },
        )
    }

    @Test fun fiveRowsThenMore() {
        val many = (1..12).map { m("opencode", "m$it") }
        val (shown, hidden) = ModelPickerRules.visible(many, expanded = false)
        assertEquals(5, shown.size)
        assertEquals(7, hidden)
        assertEquals(12 to 0, ModelPickerRules.visible(many, expanded = true).let { it.first.size to it.second })
        assertEquals(5 to 0, ModelPickerRules.visible(many.take(5), expanded = false).let { it.first.size to it.second })
        assertFalse(ModelPickerRules.startsExpanded(many, FavoriteModel("opencode", "m5")))
        assertTrue(ModelPickerRules.startsExpanded(many, FavoriteModel("opencode", "m6")))
        assertFalse(ModelPickerRules.startsExpanded(many, null))
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
