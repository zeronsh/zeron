package sh.zeron.android.ui

import org.junit.Assert.assertEquals
import org.junit.Test
import uniffi.zeron_core.ModelOption
import uniffi.zeron_core.ModelOptionChoice

class ModelOptionsTest {
    private val tier = ModelOption("serviceTier", "Service Tier", listOf(ModelOptionChoice("default", "Standard"), ModelOptionChoice("fast", "Fast")), "default")
    private val codex = ModelChoice("codex", "Codex", "gpt-6.1-sol", "GPT-6.1-Sol", listOf("low", "high"), listOf(tier))

    @Test
    fun onlyOfferedNonDefaultPicksAreSent() {
        assertEquals(mapOf("serviceTier" to "fast"), optionsFor(codex, mapOf("serviceTier" to "fast", "contextWindow" to "1m")))
        assertEquals(emptyMap<String, String>(), optionsFor(codex, mapOf("serviceTier" to "default")))
        assertEquals(emptyMap<String, String>(), optionsFor(codex, mapOf("serviceTier" to "ludicrous")))
        assertEquals(emptyMap<String, String>(), optionsFor(null, mapOf("serviceTier" to "fast")))
    }
}
