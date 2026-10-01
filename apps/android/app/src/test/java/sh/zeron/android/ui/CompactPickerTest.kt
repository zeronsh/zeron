package sh.zeron.android.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import sh.zeron.android.core.FavoriteModel
import uniffi.zeron_core.ModelInfo
import uniffi.zeron_core.ModelOption
import uniffi.zeron_core.ModelOptionChoice

private fun choiceOf(id: String, label: String) = ModelOptionChoice(id, label)

private fun toggle(id: String, default: String = "off") =
    ModelOption(id, id, listOf(choiceOf("off", "Off"), choiceOf("on", "On")), default)

private fun tier(default: String = "default") =
    ModelOption("serviceTier", "Service Tier", listOf(choiceOf("default", "Standard"), choiceOf("fast", "Fast")), default)

private fun contextWindow(default: String = "200k") =
    ModelOption("contextWindow", "Context Window", listOf(choiceOf("200k", "200K"), choiceOf("1m", "1M")), default)

private fun model(
    id: String,
    levels: List<String> = emptyList(),
    options: List<ModelOption> = emptyList(),
    default: String? = null,
    label: String = id,
    description: String? = null,
) = ModelInfo(id, label, description, levels, options, default)

private fun entry(harness: String, id: String, label: String = id, description: String? = null) =
    ModelChoice(harness, harness.uppercase(), model(id, label = label, description = description))

class EffortScaleTest {
    @Test fun stopsSitOnTheRailAndClamp() {
        assertEquals(0f, EffortScale.fraction(0, 5), 0f)
        assertEquals(0.5f, EffortScale.fraction(2, 5), 0f)
        assertEquals(1f, EffortScale.fraction(4, 5), 0f)
        assertEquals(1f, EffortScale.fraction(9, 5), 0f)
        assertEquals(0f, EffortScale.fraction(0, 1), 0f)
        assertEquals(0, EffortScale.nearestStep(0.9f, 1))
        assertEquals(0, EffortScale.nearestStep(-1f, 4))
        assertEquals(3, EffortScale.nearestStep(2f, 4))
        assertEquals(2, EffortScale.nearestStep(0.6f, 4))
    }

    @Test fun pointerMapsOntoTheThumbsTravel() {
        // 300 wide, thumb 40 wide: its centre travels 20…280.
        assertEquals(0f, EffortScale.fractionAt(0f, 300f, 20f), 0f)
        assertEquals(0f, EffortScale.fractionAt(20f, 300f, 20f), 0f)
        assertEquals(0.5f, EffortScale.fractionAt(150f, 300f, 20f), 1e-6f)
        assertEquals(1f, EffortScale.fractionAt(999f, 300f, 20f), 0f)
        assertEquals(0f, EffortScale.fractionAt(10f, 30f, 20f), 0f)
    }

    @Test fun snappingIsStickyAroundTheCurrentStop() {
        // Five stops: 0, .25, .5, .75, 1. The midpoint between stop 1 and 2 is .375.
        assertEquals(1, EffortScale.snap(0.375f, 5, current = 1))
        assertEquals(1, EffortScale.snap(0.40f, 5, current = 1))
        // Past the midpoint, plus the hysteresis margin, it moves.
        assertEquals(2, EffortScale.snap(0.43f, 5, current = 1))
        // …and coming back needs the same margin on the other side.
        assertEquals(2, EffortScale.snap(0.36f, 5, current = 2))
        assertEquals(1, EffortScale.snap(0.31f, 5, current = 2))
        // A long jump goes straight to the nearest stop.
        assertEquals(4, EffortScale.snap(1f, 5, current = 0))
        assertEquals(0, EffortScale.snap(0.2f, 1, current = 0))
    }

    @Test fun jitterOnABoundaryFiresOnce() {
        val tracker = DetentTracker(count = 5, start = 1)
        val fired = mutableListOf<Int>()
        // A finger wobbling around the 1↔2 midpoint (0.375).
        for (f in listOf(0.30f, 0.37f, 0.38f, 0.36f, 0.39f, 0.37f, 0.42f, 0.44f, 0.40f, 0.43f, 0.38f, 0.41f)) {
            tracker.update(f)?.let { fired += it }
        }
        assertEquals(listOf(2), fired)
        assertEquals(2, tracker.step)
    }

    @Test fun oneDetentPerStopCrossedNotPerFrame() {
        val tracker = DetentTracker(count = 5, start = 0)
        val fired = mutableListOf<Int>()
        // A smooth drag from the first stop to the last, in many small frames.
        var f = 0f
        while (f <= 1f) {
            tracker.update(f)?.let { fired += it }
            f += 0.005f
        }
        assertEquals(listOf(1, 2, 3, 4), fired)
        // Dragging back down fires each stop once more.
        fired.clear()
        while (f >= 0f) {
            tracker.update(f)?.let { fired += it }
            f -= 0.005f
        }
        assertEquals(listOf(3, 2, 1, 0), fired)
    }

    @Test fun staysQuietWhileNothingMoves() {
        val tracker = DetentTracker(count = 4, start = 2)
        repeat(20) { assertNull(tracker.update(2f / 3f)) }
        tracker.jump(0)
        assertNull(tracker.update(0.05f))
        assertEquals(0, DetentTracker(count = 1, start = 5).step)
    }

    @Test fun flingsCarryToTheNextStop() {
        // At rest a release stays; a quick flick to the right projects onward.
        assertEquals(1, EffortScale.landing(0.27f, 0f, 5, current = 1))
        assertEquals(2, EffortScale.landing(0.27f, 1.2f, 5, current = 1))
        assertEquals(0, EffortScale.landing(0.27f, -3f, 5, current = 1))
        assertEquals(4, EffortScale.landing(0.9f, 5f, 5, current = 3))
    }

    @Test fun endsAreFirmer() {
        assertTrue(EffortScale.isEnd(0, 5))
        assertTrue(EffortScale.isEnd(4, 5))
        assertFalse(EffortScale.isEnd(2, 5))
        assertFalse(EffortScale.isEnd(0, 1))
    }

    @Test fun onlyTheTopOfTheLadderShimmers() {
        for (low in listOf("low", "medium", "high", "minimal")) assertEquals(0f, EffortScale.energy(low), 0f)
        assertTrue(EffortScale.energy("xhigh") > 0f)
        assertTrue(EffortScale.energy("max") > EffortScale.energy("xhigh"))
        assertEquals(1f, EffortScale.energy("ultrathink"), 0f)
        assertEquals(1f, EffortScale.energy("UltraCode"), 0f)
    }
}

class FastModeTest {
    @Test fun aToggleOrATierOffersFast() {
        assertEquals(FastMode("fastMode", "on", "off"), ModelOptions.fastMode(toggle("fastMode")))
        assertEquals(FastMode("fast_mode", "on", "off"), ModelOptions.fastMode(toggle("fast_mode")))
        assertEquals(FastMode("serviceTier", "fast", "default"), ModelOptions.fastMode(tier()))
    }

    @Test fun otherOptionsAreNotFast() {
        assertNull(ModelOptions.fastMode(toggle("thinking")))
        assertNull(ModelOptions.fastMode(contextWindow()))
        // On by default: nothing to switch on.
        assertNull(ModelOptions.fastMode(toggle("fastMode", default = "on")))
        assertNull(ModelOptions.fastMode(tier(default = "fast")))
        // A "fastMode" without an on choice falls through to the tier shape.
        assertNull(ModelOptions.fastMode(ModelOption("fastMode", "Fast", listOf(choiceOf("yes", "Yes"), choiceOf("no", "No")), "no")))
    }

    @Test fun theFirstFastShapedOptionWins() {
        val m = model("m", options = listOf(contextWindow(), tier(), toggle("fastMode")))
        assertEquals("serviceTier", ModelOptions.fastMode(m)?.optionId)
        assertNull(ModelOptions.fastMode(model("m", options = listOf(contextWindow()))))
        assertNull(ModelOptions.fastMode(null))
    }

    @Test fun tappingFlipsTheCurrentState() {
        val m = model("m", options = listOf(toggle("fastMode")))
        assertFalse(ModelOptions.isFast(m, emptyMap()))
        assertEquals("fastMode" to "on", ModelOptions.fastToggle(m, emptyMap()))
        val on = mapOf("fastMode" to "on")
        assertTrue(ModelOptions.isFast(m, on))
        assertEquals("fastMode" to "off", ModelOptions.fastToggle(m, on))
        val t = model("t", options = listOf(tier()))
        assertEquals("serviceTier" to "fast", ModelOptions.fastToggle(t, emptyMap()))
        assertEquals("serviceTier" to "default", ModelOptions.fastToggle(t, mapOf("serviceTier" to "fast")))
        assertNull(ModelOptions.fastToggle(model("none"), emptyMap()))
    }
}

class OptionRowsTest {
    @Test fun effortAndFastHaveTheirOwnControls() {
        assertFalse(ModelOptions.visible(ModelSetting.Reasoning, fastId = null))
        assertFalse(ModelOptions.visible(ModelSetting.Option("fastMode"), fastId = "fastMode"))
        assertTrue(ModelOptions.visible(ModelSetting.Option("contextWindow"), fastId = "fastMode"))
        assertTrue(ModelOptions.visible(ModelSetting.Option("fastMode"), fastId = null))
    }

    @Test fun rowsAreTheRemainingOptionsWithAChoice() {
        val m = model("opus", options = listOf(contextWindow(), toggle("fastMode")))
        assertEquals(listOf("contextWindow"), ModelOptions.rows(m).map { it.id })
        val haiku = model("haiku", options = listOf(toggle("thinking")))
        assertEquals(listOf("thinking"), ModelOptions.rows(haiku).map { it.id })
        // One choice is nothing to choose between.
        val single = model("s", options = listOf(ModelOption("mode", "Mode", listOf(choiceOf("a", "A")), "a")))
        assertEquals(emptyList<ModelOption>(), ModelOptions.rows(single))
        assertEquals(emptyList<ModelOption>(), ModelOptions.rows(null))
    }

    @Test fun valuesShowThePickOrTheDefault() {
        val cw = contextWindow()
        assertEquals("200K", ModelOptions.valueLabel(cw, emptyMap()))
        assertEquals("1M", ModelOptions.valueLabel(cw, mapOf("contextWindow" to "1m")))
        // A pick the model no longer offers reads as the default.
        assertEquals("200K", ModelOptions.valueLabel(cw, mapOf("contextWindow" to "9m")))
    }

    @Test fun choosingTheDefaultClearsThePick() {
        val cw = contextWindow()
        val picked = ModelOptions.with(emptyMap(), cw, "1m")
        assertEquals(mapOf("contextWindow" to "1m"), picked)
        assertEquals(emptyMap<String, String>(), ModelOptions.with(picked, cw, "200k"))
        assertEquals(mapOf("a" to "1", "contextWindow" to "1m"), ModelOptions.with(mapOf("a" to "1"), cw, "1m"))
    }

    @Test fun effortFallsBackToTheModelsDefault() {
        val levels = listOf("low", "medium", "high", "max")
        val m = model("m", levels, default = "high")
        assertEquals("high", ModelOptions.effort(m, null))
        assertEquals("max", ModelOptions.effort(m, "max"))
        assertEquals("high", ModelOptions.effort(m, "ultra"))
        // No default reported: the middle of the ladder.
        assertEquals("high", ModelOptions.effort(model("m", levels), null))
        assertNull(ModelOptions.effort(model("m"), "high"))
        assertNull(ModelOptions.effort(null, "high"))
    }

    @Test fun resetAppearsOnlyOffDefaults() {
        val m = model("m", listOf("low", "medium", "high"), listOf(contextWindow(), toggle("fastMode")), default = "high")
        assertFalse(ModelOptions.differsFromDefaults(m, null, emptyMap()))
        assertFalse(ModelOptions.differsFromDefaults(m, "high", emptyMap()))
        assertTrue(ModelOptions.differsFromDefaults(m, "low", emptyMap()))
        assertTrue(ModelOptions.differsFromDefaults(m, null, mapOf("fastMode" to "on")))
        assertTrue(ModelOptions.differsFromDefaults(m, "high", mapOf("contextWindow" to "1m")))
        // Stale picks (defaults, or options the model doesn't have) don't count.
        assertFalse(ModelOptions.differsFromDefaults(m, null, mapOf("contextWindow" to "200k", "gone" to "x")))
        assertFalse(ModelOptions.differsFromDefaults(m, null, mapOf("contextWindow" to "bogus")))
        assertFalse(ModelOptions.differsFromDefaults(null, "low", mapOf("a" to "b")))
    }
}

class ModelListTest {
    private val catalog = listOf(
        entry("claude-code", "opus", "Opus 5"),
        entry("claude-code", "sonnet", "Sonnet 5"),
        entry("codex", "gpt-5", "GPT-5.4"),
        entry("codex", "gpt-5-mini", "GPT-5.1 Codex Mini"),
        entry("opencode", "glm", "GLM 5", description = "Zhipu via Anthropic-compatible API"),
    )

    private fun ids(entries: List<ModelPickerRules.Entry>) = entries.map { it.choice.model.id }

    @Test fun catalogOrderWhenNothingIsStarred() {
        assertEquals(listOf("opus", "sonnet", "gpt-5", "gpt-5-mini", "glm"), ids(ModelPickerRules.entries(catalog, emptyList(), "", null, false)))
    }

    @Test fun starredModelsComeFirstInCatalogOrder() {
        // Starred in a different order than the catalog: the list still reads in catalog order.
        val favorites = listOf(FavoriteModel("opencode", "glm"), FavoriteModel("claude-code", "sonnet"))
        val rows = ModelPickerRules.entries(catalog, favorites, "", null, false)
        assertEquals(listOf("sonnet", "glm", "opus", "gpt-5", "gpt-5-mini"), ids(rows))
        assertEquals(listOf(true, true, false, false, false), rows.map { it.starred })
    }

    @Test fun starringKeepsRowIdentityByKey() {
        val before = ModelPickerRules.entries(catalog, emptyList(), "", null, false).associateBy { it.key }
        val after = ModelPickerRules.entries(catalog, listOf(FavoriteModel("codex", "gpt-5")), "", null, false).associateBy { it.key }
        assertEquals(before.keys, after.keys)
        assertTrue(after.getValue("codex/gpt-5").starred)
        assertEquals(5, after.keys.size)
    }

    @Test fun searchMatchesNameAndProvider() {
        assertEquals(listOf("opus", "sonnet", "gpt-5"), ids(ModelPickerRules.entries(catalog, emptyList(), "5", null, false)).take(3))
        // The provider's name lists its models (a name hit, "Codex Mini", ranks above a provider-only hit).
        assertEquals(listOf("gpt-5-mini", "gpt-5"), ids(ModelPickerRules.entries(catalog, emptyList(), "codex", null, false)))
        assertEquals(listOf("gpt-5-mini", "gpt-5"), ids(ModelPickerRules.entries(catalog, emptyList(), "  CODEX ", null, false)))
        assertEquals(listOf("opus", "sonnet"), ids(ModelPickerRules.entries(catalog, emptyList(), "claude", null, false)))
        // A description's attribution finds a model too, ranked below name hits.
        assertEquals(listOf("glm"), ids(ModelPickerRules.entries(catalog, emptyList(), "zhipu", null, false)))
        assertEquals(emptyList<String>(), ids(ModelPickerRules.entries(catalog, emptyList(), "no such model", null, false)))
    }

    @Test fun searchRanksPrefixesBeforeSubstringsAndStarsBeforeRank() {
        val cat = listOf(entry("a", "x1", "Super Sonnet"), entry("a", "x2", "Sonnet 5"), entry("a", "x3", "Mini Sonnet"))
        assertEquals(listOf("x2", "x1", "x3"), ids(ModelPickerRules.entries(cat, emptyList(), "sonnet", null, false)))
        // A starred substring match outranks an unstarred prefix match.
        assertEquals(listOf("x3", "x2", "x1"), ids(ModelPickerRules.entries(cat, listOf(FavoriteModel("a", "x3")), "sonnet", null, false)))
    }

    @Test fun aLockedSessionListsOnlyItsOwnProvider() {
        val current = catalog[2]
        assertEquals(listOf("gpt-5", "gpt-5-mini"), ids(ModelPickerRules.entries(catalog, emptyList(), "", current, locked = true)))
        assertEquals(5, ModelPickerRules.entries(catalog, emptyList(), "", current, locked = false).size)
    }

    @Test fun anUnlistedSelectionIsShownButNotPickable() {
        val custom = entry("claude-code", "opus-preview", "Opus Preview")
        val rows = ModelPickerRules.entries(catalog, emptyList(), "", custom, locked = true)
        assertEquals(listOf("opus-preview", "opus", "sonnet"), ids(rows))
        assertTrue(rows.first().selectedOnly)
        assertFalse(rows.drop(1).any { it.selectedOnly })
        // While the catalog is still loading it is the only row.
        assertEquals(listOf("opus-preview"), ids(ModelPickerRules.entries(emptyList(), emptyList(), "", custom, locked = true)))
        // A search it doesn't match hides it; one it matches keeps it.
        assertEquals(emptyList<String>(), ids(ModelPickerRules.entries(catalog, emptyList(), "gpt", custom, locked = true)))
        assertEquals(listOf("opus-preview", "opus"), ids(ModelPickerRules.entries(catalog, emptyList(), "opus", custom, locked = true)))
        // A listed selection is not duplicated.
        assertEquals(5, ModelPickerRules.entries(catalog, emptyList(), "", catalog[0], false).size)
    }

    @Test fun onlyRepeatedNamesNeedTheirDescription() {
        val rows = ModelPickerRules.entries(
            listOf(entry("a", "x1", "Sonnet", "via A"), entry("b", "x2", "sonnet ", "via B"), entry("a", "x3", "Opus", "plain")),
            emptyList(), "", null, false,
        )
        assertEquals(setOf("Sonnet", "sonnet "), ModelPickerRules.ambiguousLabels(rows))
        assertEquals(emptySet<String>(), ModelPickerRules.ambiguousLabels(rows.takeLast(1)))
    }

    @Test fun opensOnTheSelection() {
        assertEquals(0, ModelPickerRules.initialScrollIndex(-1))
        assertEquals(0, ModelPickerRules.initialScrollIndex(0))
        assertEquals(0, ModelPickerRules.initialScrollIndex(4))
        assertEquals(7, ModelPickerRules.initialScrollIndex(10, visibleRows = 6))
        assertEquals(2, ModelPickerRules.initialScrollIndex(3, visibleRows = 2))
    }

    @Test fun aMovedRowIsFollowedOnlyWhenItLeavesTheView() {
        assertNull(ModelPickerRules.followScroll(newIndex = 4, firstVisible = 2, lastVisible = 7))
        assertEquals(0, ModelPickerRules.followScroll(newIndex = 0, firstVisible = 3, lastVisible = 8))
        assertEquals(12, ModelPickerRules.followScroll(newIndex = 12, firstVisible = 3, lastVisible = 8))
        assertNull(ModelPickerRules.followScroll(newIndex = -1, firstVisible = 0, lastVisible = 5))
    }
}

class ChipTextTest {
    private val label = { id: String -> id.replaceFirstChar { it.uppercase() } }

    @Test fun modelThenDimEffort() {
        val parts = ChipText.parts("GPT-5.4", "high", false, label)
        assertEquals(ChipText.Parts("GPT-5.4", "High", false), parts)
        assertEquals("Model: GPT-5.4, High effort", ChipText.describe(parts))
    }

    @Test fun noLadderNoEffort() {
        assertEquals(ChipText.Parts("Haiku 4.5", null, false), ChipText.parts("Haiku 4.5", null, false, label))
        assertNull(ChipText.parts("Haiku 4.5", "", false, label).effort)
        assertEquals("Model: Haiku 4.5", ChipText.describe(ChipText.parts("Haiku 4.5", null, false, label)))
    }

    @Test fun fastModeIsAnnounced() {
        assertEquals("Model: Opus 5, Max effort, fast mode on", ChipText.describe(ChipText.parts("Opus 5", "max", true, label)))
    }
}
