package sh.zeron.android.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.zeron_core.ServerInfo

class ServerTest {
    @Test
    fun a_launch_argument_wins_signed_in_or_out() {
        assertEquals("launch", pickServer("launch", signedIn = true, account = "account", chosen = "chosen", official = "official"))
        assertEquals("launch", pickServer("launch", signedIn = false, account = null, chosen = "chosen", official = "official"))
    }

    @Test
    fun signed_in_uses_the_accounts_server_never_the_chosen_one() {
        assertEquals("account", pickServer(null, signedIn = true, account = "account", chosen = "chosen", official = "official"))
        // Credentials stored without a server are on Zeron's, whatever was chosen since.
        assertEquals("official", pickServer(null, signedIn = true, account = null, chosen = "chosen", official = "official"))
    }

    @Test
    fun signed_out_uses_the_chosen_server_else_zerons() {
        assertEquals("chosen", pickServer(null, signedIn = false, account = "account", chosen = "chosen", official = "official"))
        assertEquals("official", pickServer(null, signedIn = false, account = null, chosen = null, official = "official"))
    }

    @Test
    fun zerons_server_keeps_the_owner_existing_caches_have() {
        assertEquals("user_1/org_1", coreOwner(null, "user_1", "org_1"))
    }

    @Test
    fun a_self_hosted_owner_includes_the_server() {
        assertEquals("https://zeron.acme.dev|user_1/org_1", coreOwner("https://zeron.acme.dev", "user_1", "org_1"))
    }

    @Test
    fun the_chosen_server_round_trips() {
        val acme = StoredServer("https://zeron.acme.dev", "client_acme", "Acme")
        assertEquals(acme, StoredServer.fromJson(acme.toJson()))
        assertEquals(acme.copy(name = null), StoredServer.fromJson(acme.copy(name = null).toJson()))
    }

    @Test
    fun no_or_broken_chosen_server_is_zerons() {
        assertNull(StoredServer.fromJson(null))
        assertNull(StoredServer.fromJson("not json"))
        assertNull(StoredServer.fromJson("""{"url":"https://zeron.acme.dev"}"""))
        assertNull(StoredServer.fromJson("""{"workosClientId":"client_acme"}"""))
    }

    @Test
    fun zerons_server_is_stored_as_nothing() {
        assertNull(StoredServer.of(info("https://edge.zeron.sh", "client_zeron", official = true)))
        assertEquals(
            StoredServer("https://zeron.acme.dev", "client_acme", "Acme"),
            StoredServer.of(info("https://zeron.acme.dev", "client_acme", name = "Acme")),
        )
    }

    @Test
    fun the_same_server_is_one_address_and_one_sign_in() {
        val acme = info("https://zeron.acme.dev", "client_acme", name = "Acme")
        assertTrue(acme.isSameAs(info("https://zeron.acme.dev", "client_acme", name = "Acme, renamed")))
        assertFalse(acme.isSameAs(info("https://zeron.acme.dev", "client_other")))
        assertFalse(acme.isSameAs(info("https://other.acme.dev", "client_acme")))
    }

    private fun info(url: String, client: String, name: String? = null, official: Boolean = false): ServerInfo {
        val host = url.substringAfter("://")
        return ServerInfo(url, client, name, host, name ?: host, official)
    }
}
