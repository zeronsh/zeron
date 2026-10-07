package sh.zeron.android.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertSame
import org.junit.Test
import uniffi.zeron_core.CatalogSource
import uniffi.zeron_core.HarnessCatalog
import uniffi.zeron_core.HarnessInfo
import uniffi.zeron_core.ModelCatalog
import uniffi.zeron_core.ModelInfo

class HostCatalogTest {
    private fun c(h: String, id: String) = ModelChoice(h, h, id, id, emptyList())

    private val base = HostCatalog(
        models = listOf(c("claude-code", "opus"), c("codex", "gpt-6-astra"), c("codex", "gpt-5.5"), c("pi", "pi-1")),
        stale = mapOf("pi" to CatalogSource.STATIC),
    )

    private fun harness(id: String, offered: Boolean = true) =
        HarnessInfo(id, id, null, null, emptyList(), offered, null, offered, null)

    private fun info(id: String) = ModelInfo(id, id, null, emptyList(), emptyList(), null)

    @Test
    fun aLiveRefreshReplacesOneHarnessInPlaceAndClearsItsStaleMark() {
        val fresh = listOf(c("pi", "pi-2"), c("pi", "pi-3"))
        val next = base.with("pi", fresh, CatalogSource.LIVE)
        assertEquals(listOf("opus", "gpt-6-astra", "gpt-5.5", "pi-2", "pi-3"), next.models.map { it.id })
        assertEquals(emptyMap<String, CatalogSource>(), next.stale)
    }

    @Test
    fun aSavedListIsTheComputersOwnAndGetsNoRetryRow() {
        val next = base.with("pi", listOf(c("pi", "pi-saved")), CatalogSource.SAVED, "host unavailable: timed out")
        assertEquals("pi-saved", next.models.last().id)
        assertEquals(emptyMap<String, CatalogSource>(), next.stale)
        assertEquals(emptyMap<String, String>(), next.errors)
    }

    @Test
    fun aFailedReadNeverSwapsTheComputersListForTheBuiltInOne() {
        val next = base.with("codex", listOf(c("codex", "gpt-daybreak-blue-latest")), CatalogSource.STATIC, "timed out")
        assertSame(base, next)
    }

    @Test
    fun aHarnessNotListedYetGoesLast() {
        val next = base.with("devin", listOf(c("devin", "swe-1")), CatalogSource.LIVE)
        assertEquals("swe-1", next.models.last().id)
    }

    @Test
    fun aFailedReadKeepsItsReasonUntilALiveListArrives() {
        val failed = base.with("pi", listOf(c("pi", "pi-0")), CatalogSource.STATIC, "host unavailable: ListHarnesses failed (timed out)")
        assertEquals(mapOf("pi" to "host unavailable: ListHarnesses failed (timed out)"), failed.errors)
        assertEquals("ListHarnesses failed (timed out)", HostCatalog.reason(failed.errors.getValue("pi")))
        val live = failed.with("pi", listOf(c("pi", "pi-9")), CatalogSource.LIVE)
        assertEquals(emptyMap<String, String>(), live.errors)
    }

    @Test
    fun aLongReasonIsShortened() {
        val reason = HostCatalog.reason("x".repeat(300))
        assertEquals(120, reason.length)
        assertEquals('…', reason.last())
    }

    @Test
    fun theSheetOpensOnTheSavedListsWithARetryRowOnlyForNeverReadClis() {
        val opened = HostCatalog.fromSaved(
            listOf(
                HarnessCatalog(harness("codex"), ModelCatalog(listOf(info("gpt-6.1-sol"), info("gpt-6-astra")), CatalogSource.SAVED, null)),
                HarnessCatalog(harness("pi"), ModelCatalog(listOf(info("pi-default")), CatalogSource.STATIC, null)),
                HarnessCatalog(harness("hermes", offered = false), ModelCatalog(listOf(info("h-4")), CatalogSource.SAVED, null)),
            ),
        )
        assertEquals(listOf("gpt-6.1-sol", "gpt-6-astra", "pi-default"), opened.models.map { it.id })
        assertEquals(mapOf("pi" to CatalogSource.STATIC), opened.stale)
    }

    @Test
    fun aBackgroundReadThatFailedKeepsTheShownListsAndALiveOneReplacesThem() {
        val shown = HostCatalog(listOf(c("claude-code", "opus"), c("codex", "gpt-6.1-sol"), c("pi", "pi-1")), mapOf("pi" to CatalogSource.STATIC))
        // The computer didn't answer: Codex would fall back to the built-in list.
        val failed = HostCatalog(
            listOf(c("claude-code", "opus-live"), c("codex", "gpt-daybreak-blue-latest"), c("pi", "pi-0")),
            stale = mapOf("codex" to CatalogSource.STATIC, "pi" to CatalogSource.STATIC),
            errors = mapOf("codex" to "timed out", "pi" to "timed out"),
        )
        val merged = shown.merge(failed)
        assertEquals(listOf("opus-live", "gpt-6.1-sol", "pi-0"), merged.models.map { it.id })
        assertEquals(mapOf("pi" to CatalogSource.STATIC), merged.stale)
        assertEquals(mapOf("pi" to "timed out"), merged.errors)
        // A CLI the computer no longer offers drops out; a live list clears the row.
        val live = merged.merge(HostCatalog(listOf(c("codex", "gpt-7"), c("pi", "pi-live"))))
        assertEquals(listOf("gpt-7", "pi-live"), live.models.map { it.id })
        assertEquals(emptyMap<String, CatalogSource>(), live.stale)
    }
}
