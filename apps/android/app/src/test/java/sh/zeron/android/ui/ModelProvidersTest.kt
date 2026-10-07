package sh.zeron.android.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Test
import uniffi.zeron_core.CatalogSource
import uniffi.zeron_core.ModelInfo

/**
 * The provider under a model row (desktop pickers.rs `mark_ambiguous`,
 * widened: same name OR same model under another provider) and the model
 * list's forced refresh, which never downgrades what shows.
 */
class ModelProvidersTest {
    private fun info(id: String, label: String, description: String? = null) =
        ModelInfo(id, label, description, emptyList(), emptyList(), null)

    private fun pi(id: String, label: String, description: String? = null) =
        ModelChoice.of("pi", "Pi", info(id, label, description))

    /** pi with providers AG.20 and AG.50 serving the same models. */
    private val piRows = listOf(
        pi("AG.20/gpt-6-astra", "GPT-6 Astra"),
        pi("AG.20/gpt-6.1-sol", "GPT-6.1 Sol"),
        pi("AG.50/gpt-6-astra", "GPT-6 Astra"),
        pi("AG.50/gpt-6.1-sol", "GPT-6.1 Sol"),
    )

    @Test
    fun theProviderIsTheDescriptionElseTheIdPrefix() {
        assertEquals("AG.20", pi("AG.20/gpt-6-astra", "GPT-6 Astra").provider)
        assertEquals("Z.AI", ModelChoice.of("opencode", "OpenCode", info("zai/glm-5.2", "GLM-5.2", "Z.AI")).provider)
        // A description that only repeats the harness name says nothing.
        assertEquals("zai", ModelChoice.of("opencode", "OpenCode", info("zai/glm-5.2", "GLM-5.2", " OpenCode ")).provider)
        assertNull(ModelChoice.of("codex", "Codex", info("gpt-6-astra", "GPT-6-Astra")).provider)
        assertEquals("gpt-6-astra", pi("AG.20/gpt-6-astra", "x").baseId)
        assertEquals("gpt-5.5", ModelChoice.of("codex", "Codex", info("gpt-5.5", "GPT-5.5")).baseId)
    }

    @Test
    fun sameNamedRowsInOneHarnessShowTheirProviders() {
        val ambiguous = ambiguousRows(piRows)
        assertEquals(listOf("AG.20", "AG.20", "AG.50", "AG.50"), piRows.map { it.providerLine(ambiguous) })
        assertEquals("GPT-6 Astra · AG.50", piRows[2].titleAmong(ambiguous))
    }

    @Test
    fun theSameModelUnderAnotherProviderShowsItEvenWithDifferentNames() {
        // The user's config names them "Astra ($20)" / "Astra ($50)": still variants of one model.
        val rows = listOf(pi("AG.20/gpt-6-astra", "Astra (\$20)"), pi("AG.50/gpt-6-astra", "Astra (\$50)"), pi("AG.20/gpt-6.1-sol", "Sol"))
        val ambiguous = ambiguousRows(rows)
        assertEquals(listOf("AG.20", "AG.50", null), rows.map { it.providerLine(ambiguous) })
        assertEquals("Sol", rows[2].titleAmong(ambiguous))
    }

    @Test
    fun uniqueRowsAndOtherHarnessesStayBare() {
        val rows = listOf(
            ModelChoice.of("codex", "Codex", info("gpt-6-astra", "GPT-6 Astra", "Our most capable model")),
            pi("AG.20/gpt-6-astra", "GPT-6 Astra"),
            ModelChoice.of("claude-code", "Claude Code", info("opus", "Opus", "Most capable")),
        )
        val ambiguous = ambiguousRows(rows)
        // Same name and model across harnesses isn't a collision: each harness has its own list.
        assertEquals(emptySet<Pair<String, String>>(), ambiguous)
        assertEquals(listOf(null, null, null), rows.map { it.providerLine(ambiguous) })
        assertEquals("GPT-6 Astra", rows[1].titleAmong(ambiguous))
    }

    private val shown = HostCatalog(
        listOf(ModelChoice.of("codex", "Codex", info("gpt-6.1-sol", "GPT-6.1-Sol"))) + piRows,
        stale = mapOf("devin" to CatalogSource.STATIC),
    )

    @Test
    fun aLiveRefreshReplacesTheHarnessList() {
        val fresh = HarnessList(piRows + pi("AG.50/gpt-6.2-nova", "GPT-6.2 Nova"), CatalogSource.LIVE, null)
        val (next, failure) = shown.refreshed("pi", fresh)
        assertNull(failure)
        assertEquals("AG.50/gpt-6.2-nova", next.models.last().id)
        assertEquals("gpt-6.1-sol", next.models.first().id)
    }

    @Test
    fun aFailedRefreshKeepsWhatShowsAndSaysWhy() {
        // The core answers with its saved copy (or the built-in list) and the reason.
        val saved = HarnessList(piRows.take(1), CatalogSource.SAVED, "host error: pi: provider AG.50 unreachable")
        val (keptSaved, why) = shown.refreshed("pi", saved)
        assertSame(shown, keptSaved)
        assertEquals("pi: provider AG.50 unreachable", HostCatalog.reason(why!!))
        val builtIn = HarnessList(listOf(pi("default", "pi default")), CatalogSource.STATIC, "timed out")
        assertSame(shown, shown.refreshed("pi", builtIn).first)
        // The call itself threw.
        val (keptThrown, thrown) = shown.refreshed("pi", null, "link down")
        assertSame(shown, keptThrown)
        assertEquals("link down", thrown)
    }

    @Test
    fun aRetryOnTheBuiltInListTakesTheSavedOneOrKeepsItsReason() {
        val stale = HostCatalog(shown.models + ModelChoice.of("devin", "Devin", info("swe-1", "SWE-1")), stale = mapOf("devin" to CatalogSource.STATIC))
        val (failed, why) = stale.refreshed("devin", HarnessList(listOf(ModelChoice.of("devin", "Devin", info("swe-1", "SWE-1"))), CatalogSource.STATIC, "timed out"))
        assertEquals("timed out", why)
        assertEquals(mapOf("devin" to "timed out"), failed.errors)
        assertEquals(mapOf("devin" to CatalogSource.STATIC), failed.stale)
        val (saved, _) = stale.refreshed("devin", HarnessList(listOf(ModelChoice.of("devin", "Devin", info("swe-2", "SWE-2"))), CatalogSource.SAVED, "timed out"))
        assertEquals(emptyMap<String, CatalogSource>(), saved.stale)
        assertEquals("swe-2", saved.models.last().id)
    }
}
