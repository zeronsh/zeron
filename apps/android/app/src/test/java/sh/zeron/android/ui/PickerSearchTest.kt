package sh.zeron.android.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Test

private fun choice(label: String, fixed: Boolean = false, supporting: String? = null) =
    MenuChoice(label, supporting = supporting, fixed = fixed) {}

class PickerSearchTest {
    @Test fun blankQueryMatchesEverythingInOrder() {
        val rows = listOf("b", "a", "c")
        assertEquals(rows, PickerSearch.filter(rows, "  ", { it }))
    }

    @Test fun ranksExactThenPrefixThenWordStartThenAnywhere() {
        val rows = listOf("my opus", "xopusx", "Opus 4.5", "opus")
        assertEquals(listOf("opus", "Opus 4.5", "my opus", "xopusx"), PickerSearch.filter(rows, "opus", { it }))
    }

    @Test fun ignoresCaseAndPunctuation() {
        assertNotNull(PickerSearch.score("gpt5", "GPT-5"))
        assertNotNull(PickerSearch.score("OPUS", "claude opus"))
        assertNotNull(PickerSearch.score("opus45", "Opus 4.5"))
    }

    @Test fun everyWordMustMatchSomewhere() {
        assertNotNull(PickerSearch.score("zeron laptop", "zeron", listOf("laptop", "~/code/zeron")))
        assertNull(PickerSearch.score("zeron desktop", "zeron", listOf("laptop")))
    }

    @Test fun secondaryFieldsRankBelowTheName() {
        val rows = listOf("api" to "~/code/zeron", "zeron" to "~/code/zeron")
        assertEquals(listOf("zeron", "api"), PickerSearch.filter(rows, "zeron", { it.first }, { listOf(it.second) }).map { it.first })
    }

    @Test fun subsequenceIsTheLastResortForLongerWords() {
        assertNotNull(PickerSearch.score("snt", "Sonnet"))
        assertNull(PickerSearch.score("zz", "Sonnet"))
        val ranked = PickerSearch.filter(listOf("Sonnet", "snt-mini"), "snt", { it })
        assertEquals(listOf("snt-mini", "Sonnet"), ranked)
    }

    @Test fun sortsByNameRecencyAndCreation() {
        data class P(val name: String, val recent: Long?, val created: Long)
        val rows = listOf(P("beta", 5, 1), P("Alpha", null, 3), P("gamma", 9, 2))
        assertEquals(rows, PickerSearch.sorted(rows, PickerSort.Default, { it.name }))
        assertEquals(listOf("Alpha", "beta", "gamma"), PickerSearch.sorted(rows, PickerSort.Name, { it.name }).map { it.name })
        assertEquals(listOf("gamma", "beta", "Alpha"), PickerSearch.sorted(rows, PickerSort.Recent, { it.name }, recent = { it.recent }).map { it.name })
        assertEquals(listOf("Alpha", "gamma", "beta"), PickerSearch.sorted(rows, PickerSort.Newest, { it.name }, created = { it.created }).map { it.name })
        // A sort the rows can't answer keeps their order.
        assertEquals(rows, PickerSearch.sorted(rows, PickerSort.Recent, { it.name }))
    }

    @Test fun sortToggleCycles() {
        val options = listOf(PickerSort.Name, PickerSort.Recent, PickerSort.Newest)
        assertEquals(PickerSort.Recent, PickerSearch.next(PickerSort.Name, options))
        assertEquals(PickerSort.Name, PickerSearch.next(PickerSort.Newest, options))
    }

    @Test fun fixedChoicesHoldTheirPlaceWhenSorted() {
        val choices = listOf(choice("All projects", fixed = true), choice("zeron"), choice("api"), choice("New…", fixed = true))
        assertEquals(
            listOf("All projects", "api", "zeron", "New…"),
            PickerSearch.arrange(choices, "", PickerSort.Name).map { it.label },
        )
        assertEquals(choices, PickerSearch.arrange(choices, "", PickerSort.Default))
        assertEquals(listOf("zeron"), PickerSearch.arrange(choices, "zer", PickerSort.Name).map { it.label })
        assertEquals(listOf("api"), PickerSearch.arrange(listOf(choice("api", supporting = "Online"), choice("web")), "online", PickerSort.Default).map { it.label })
    }

    @Test fun matchingFixedChoicesStayAtTheirEndsWhenSearched() {
        val choices = listOf(choice("All sections", fixed = true), choice("Backlog"), choice("Sections archive"), choice("No section", fixed = true), choice("New section…", fixed = true))
        // "Sections archive" is the best match, but the fixed rows keep their ends.
        assertEquals(
            listOf("All sections", "Sections archive", "No section", "New section…"),
            PickerSearch.arrange(choices, "section", PickerSort.Name).map { it.label },
        )
        assertEquals(listOf("All sections", "Sections archive"), PickerSearch.arrange(choices, "sections", PickerSort.Default).map { it.label })
    }
}
