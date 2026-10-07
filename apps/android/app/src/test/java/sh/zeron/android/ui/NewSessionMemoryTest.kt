package sh.zeron.android.ui

import org.junit.Assert.assertEquals
import org.junit.Test
import sh.zeron.android.ui.NewSessionMemory.Picks

class NewSessionMemoryTest {
    private data class P(val id: String, val lastUsed: Long)

    private val projects = listOf(P("blog", 10), P("zeron", 300), P("notes", 20))
    private val hosts = listOf("villa", "nas")

    private fun initial(explicit: String? = null, remembered: Picks? = null) =
        NewSessionMemory.initial(explicit, remembered, projects, hosts, { it.id }, { it.lastUsed })

    @Test
    fun withNothingRememberedItOpensOnTheMostRecentlyUsedProjectNotTheFirst() {
        assertEquals(Picks("zeron", null), initial())
    }

    @Test
    fun theLastPickOnThisComputerWins() {
        assertEquals(Picks("notes", null), initial(remembered = Picks("notes", null)))
    }

    @Test
    fun theProjectItWasOpenedFromWinsOverMemory() {
        assertEquals(Picks("blog", null), initial(explicit = "blog", remembered = Picks("notes", null)))
    }

    @Test
    fun aRememberedProjectThatIsGoneFallsBackToTheMostRecent() {
        assertEquals(Picks("zeron", null), initial(remembered = Picks("deleted", null)))
    }

    @Test
    fun aProjectlessPickReopensProjectlessOnItsHost() {
        assertEquals(Picks(null, "nas"), initial(remembered = Picks(null, "nas")))
        assertEquals(Picks("zeron", null), initial(remembered = Picks(null, "gone-host")))
    }
}
