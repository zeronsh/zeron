package sh.zeron.android.core

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.zeron_core.AuthTokens
import uniffi.zeron_core.Credentials
import uniffi.zeron_core.DemoFixture
import uniffi.zeron_core.DemoOptions
import uniffi.zeron_core.StreamSpeed
import uniffi.zeron_core.TranscriptScale

class CredentialJsonTest {
    private val workos = Credentials.WorkOs("user_1", "org_1", AuthTokens("access-1", "refresh-1"))
    private val acme = StoredServer("https://zeron.acme.dev", "client_acme", "Acme")

    @Test
    fun credentials_saved_before_servers_decode_unchanged_on_zerons_server() {
        val legacy = """{"kind":"workos","userId":"user_1","orgId":"org_1","access":"access-1","refresh":"refresh-1"}"""
        val account = CredentialJson.decode(legacy)!!
        assertEquals(workos, account.credentials)
        assertNull(account.server)
    }

    @Test
    fun zerons_server_writes_no_server_fields() {
        val json = JSONObject(CredentialJson.encode(workos, null)!!)
        assertFalse(json.has("edge"))
        assertFalse(json.has("workosClientId"))
        assertFalse(json.has("serverName"))
        assertEquals("access-1", json.getString("access"))
    }

    @Test
    fun a_self_hosted_server_is_written_beside_the_credentials() {
        val json = JSONObject(CredentialJson.encode(workos, acme)!!)
        assertEquals("https://zeron.acme.dev", json.getString("edge"))
        assertEquals("client_acme", json.getString("workosClientId"))
        assertEquals("Acme", json.getString("serverName"))
    }

    @Test
    fun an_unnamed_server_writes_no_name() {
        val json = JSONObject(CredentialJson.encode(workos, acme.copy(name = null))!!)
        assertTrue(json.has("edge"))
        assertFalse(json.has("serverName"))
    }

    @Test
    fun a_self_hosted_account_round_trips() {
        val account = CredentialJson.decode(CredentialJson.encode(workos, acme)!!)!!
        assertEquals(workos, account.credentials)
        assertEquals(acme, account.server)
        assertEquals(acme.copy(name = null), CredentialJson.decode(CredentialJson.encode(workos, acme.copy(name = null))!!)!!.server)
    }

    @Test
    fun half_a_server_is_no_server() {
        val edgeOnly = """{"kind":"workos","userId":"u","orgId":"o","access":"a","refresh":"r","edge":"https://zeron.acme.dev"}"""
        val clientOnly = """{"kind":"workos","userId":"u","orgId":"o","access":"a","refresh":"r","workosClientId":"client_acme"}"""
        val nulls = """{"kind":"workos","userId":"u","orgId":"o","access":"a","refresh":"r","edge":null,"workosClientId":null}"""
        assertNull(CredentialJson.decode(edgeOnly)!!.server)
        assertNull(CredentialJson.decode(clientOnly)!!.server)
        assertNull(CredentialJson.decode(nulls)!!.server)
    }

    @Test
    fun dev_credentials_keep_their_server_too() {
        val dev = Credentials.Dev("user_2", "org_2")
        assertEquals(CredentialStore.Account(dev, null), CredentialJson.decode(CredentialJson.encode(dev, null)!!))
        assertEquals(CredentialStore.Account(dev, acme), CredentialJson.decode(CredentialJson.encode(dev, acme)!!))
    }

    @Test
    fun the_demo_is_never_stored() {
        val demo = Credentials.Demo(DemoOptions(DemoFixture.STANDARD, TranscriptScale.Normal, StreamSpeed.REALISTIC, false))
        assertNull(CredentialJson.encode(demo, null))
    }

    @Test
    fun unreadable_json_is_no_account() {
        assertNull(CredentialJson.decode("not json"))
        assertNull(CredentialJson.decode("""{"kind":"other"}"""))
    }
}
