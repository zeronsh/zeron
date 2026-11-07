package sh.zeron.android.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.zeron_core.ConnectivityState
import uniffi.zeron_core.SendState
import uniffi.zeron_core.TranscriptLoad

class SessionNoticesTest {
    @Test
    fun a_chat_with_something_to_show_says_nothing() {
        assertNull(EmptyNotice.of(TranscriptLoad.READY))
    }

    @Test
    fun the_empty_states_use_the_desktops_words() {
        assertEquals("Syncing…", EmptyNotice.of(TranscriptLoad.SYNCING)?.text)
        assertEquals("Reconnecting…", EmptyNotice.of(TranscriptLoad.RECONNECTING)?.text)
        assertEquals("Offline — this chat loads when you're back online", EmptyNotice.of(TranscriptLoad.OFFLINE)?.text)
        assertEquals("This chat lives on another device — it loads when that device is back online", EmptyNotice.of(TranscriptLoad.HOST_AWAY)?.text)
    }

    @Test
    fun only_states_where_something_is_happening_get_a_spinner() {
        assertTrue(EmptyNotice.Syncing.working)
        assertTrue(EmptyNotice.Reconnecting.working)
        assertFalse(EmptyNotice.Offline.working)
        assertFalse(EmptyNotice.HostAway.working)
    }

    private fun banner(
        send: SendState? = null,
        host: String? = "Studio",
        cloud: Boolean = false,
        connectivity: ConnectivityState? = ConnectivityState.CONNECTED,
        connected: Boolean = true,
        retryAt: Long? = null,
    ) = bannerText(send, host, cloud, connectivity, connected, retryAt, nowMs = 10_000)

    @Test
    fun a_send_waiting_on_a_host_says_so() {
        assertEquals("Studio is offline — will send when it's back", banner(SendState.QUEUED))
        assertEquals("Host is offline — will send when it's back", banner(SendState.QUEUED, host = null))
    }

    @Test
    fun a_cloud_chats_send_waits_for_its_machine_without_a_banner() {
        assertNull(banner(SendState.QUEUED, cloud = true))
        assertEquals("Offline — sends are saved", banner(SendState.QUEUED, cloud = true, connectivity = ConnectivityState.OFFLINE))
        assertEquals("Offline — sends are saved", banner(SendState.QUEUED, connectivity = ConnectivityState.OFFLINE))
    }

    @Test
    fun send_state_outranks_the_connection() {
        assertEquals("Not delivered", banner(SendState.FAILED, connectivity = ConnectivityState.OFFLINE))
        assertEquals("Offline — sends are saved", banner(SendState.SENDING, connectivity = ConnectivityState.OFFLINE))
    }

    @Test
    fun offline_with_nothing_sent_says_sends_are_saved() {
        assertEquals("Offline — sends are saved", banner(connectivity = ConnectivityState.OFFLINE))
    }

    @Test
    fun a_reconnect_counts_down_and_never_says_zero() {
        assertEquals("Reconnecting in 5s", banner(connected = false, retryAt = 15_000))
        assertEquals("Reconnecting in 1s", banner(connected = false, retryAt = 9_000))
        assertNull(banner(connected = false))
    }
}
