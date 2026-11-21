package sh.zeron.android.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class ServerLinkTest {
    @Test
    fun a_host_is_the_address() {
        assertEquals("zeron.acme.com", ServerLink.address("zeron://server?address=zeron.acme.com"))
    }

    @Test
    fun a_percent_encoded_https_url_is_decoded() {
        assertEquals("https://zeron.acme.com", ServerLink.address("zeron://server?address=https%3A%2F%2Fzeron.acme.com"))
    }

    @Test
    fun an_http_url_with_a_port_is_decoded() {
        assertEquals("http://10.0.2.2:8899", ServerLink.address("zeron://server?address=http%3A%2F%2F10.0.2.2%3A8899"))
    }

    @Test
    fun the_address_is_found_among_other_parameters_and_trimmed() {
        assertEquals("zeron.acme.com", ServerLink.address("zeron://server?from=invite&address=%20zeron.acme.com%20"))
    }

    @Test
    fun no_or_a_blank_address_is_no_link() {
        assertNull(ServerLink.address("zeron://server"))
        assertNull(ServerLink.address("zeron://server?"))
        assertNull(ServerLink.address("zeron://server?address"))
        assertNull(ServerLink.address("zeron://server?address="))
        assertNull(ServerLink.address("zeron://server?address=%20%20"))
        assertNull(ServerLink.address("zeron://server?name=Acme"))
    }

    @Test
    fun another_host_or_scheme_is_not_a_server_link() {
        assertNull(ServerLink.address("zeron://callback?address=zeron.acme.com"))
        assertNull(ServerLink.address("zeron://cloud?address=zeron.acme.com"))
        assertNull(ServerLink.address("https://server?address=zeron.acme.com"))
        assertNull(ServerLink.address("https://zeron.sh/server?address=zeron.acme.com"))
    }

    @Test
    fun a_malformed_link_is_ignored() {
        assertNull(ServerLink.address("zeron://server?address=%E0%A4%A"))
        assertNull(ServerLink.address("not a link"))
        assertNull(ServerLink.address(""))
    }
}
