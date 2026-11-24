package sh.zeron.android.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import uniffi.zeron_core.ServerInfo

class ServerUiTest {
    private val acme = info("https://zeron.acme.com", name = "Acme")
    private val unnamed = info("http://10.0.2.2:8899")

    @Test
    fun a_links_server_waits_for_its_tap_by_name() {
        assertEquals("Connect to Acme", serverConfirmLabel(ServerSheetMode.SignIn, selfHosted = true, found = acme))
    }

    @Test
    fun an_unnamed_links_server_is_called_by_its_host() {
        assertEquals("Connect to 10.0.2.2:8899", serverConfirmLabel(ServerSheetMode.SignIn, selfHosted = true, found = unnamed))
    }

    @Test
    fun without_a_found_server_the_button_says_what_it_does() {
        assertEquals("Connect", serverConfirmLabel(ServerSheetMode.SignIn, selfHosted = true, found = null))
        assertEquals("Use Zeron", serverConfirmLabel(ServerSheetMode.SignIn, selfHosted = false, found = null))
        assertEquals("Continue", serverConfirmLabel(ServerSheetMode.Switch, selfHosted = true, found = null))
    }

    @Test
    fun zerons_server_has_no_identity_on_the_welcome() {
        assertNull(serverIdentity(info("https://edge.zeron.sh", official = true)))
    }

    @Test
    fun a_named_server_shows_its_name_and_host() {
        val identity = serverIdentity(acme)
        assertEquals(ServerIdentity("Acme", "zeron.acme.com"), identity)
        assertEquals("Server, Acme, zeron.acme.com", identity?.description)
    }

    @Test
    fun an_unnamed_server_shows_its_host_once() {
        val identity = serverIdentity(unnamed)
        assertEquals(ServerIdentity("10.0.2.2:8899", null), identity)
        assertEquals("Server, 10.0.2.2:8899", identity?.description)
    }

    private fun info(url: String, name: String? = null, official: Boolean = false): ServerInfo {
        val host = url.substringAfter("://")
        return ServerInfo(url, "client", name, host, name ?: host, official)
    }
}
