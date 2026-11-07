package sh.zeron.android.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class NetworkPathTrackerTest {
    private val calls = mutableListOf<String>()
    private val tracker = NetworkPathTracker(
        setOnline = { calls += if (it) "online" else "offline" },
        pathChanged = { calls += "moved" },
    )

    @Test
    fun launching_with_no_network_reads_offline_though_the_os_says_nothing() {
        tracker.seed(null)
        assertFalse(tracker.online)
        assertEquals(emptyList<String>(), calls)
    }

    @Test
    fun the_first_callback_for_the_network_already_known_changes_nothing() {
        tracker.seed("100/1")
        tracker.update("100/1")
        assertTrue(tracker.online)
        assertEquals(emptyList<String>(), calls)
    }

    @Test
    fun re_delivered_capabilities_leave_a_healthy_link_alone() {
        tracker.seed("100/1")
        repeat(5) { tracker.update("100/1") }
        assertEquals(emptyList<String>(), calls)
    }

    @Test
    fun moving_between_networks_without_losing_one_is_a_path_change() {
        tracker.seed("100/1")
        tracker.update("101/0!")
        assertEquals(listOf("moved"), calls)
    }

    @Test
    fun a_network_turning_metered_is_a_path_change() {
        tracker.seed("100/1")
        tracker.update("100/1!")
        assertEquals(listOf("moved"), calls)
    }

    @Test
    fun losing_the_network_then_getting_another_reports_once_each_way() {
        tracker.seed("100/1")
        tracker.update(null)
        tracker.update("101/0!")
        // The core treats coming back online as a path change itself.
        assertEquals(listOf("offline", "online"), calls)
    }

    @Test
    fun being_cut_off_with_the_network_still_up_counts_as_no_path() {
        tracker.seed("100/1")
        tracker.setBlocked(true)
        assertFalse(tracker.online)
        tracker.update("100/1!")
        tracker.setBlocked(false)
        assertTrue(tracker.online)
        // Nothing can be torn down twice: moving while cut off is covered by coming back.
        assertEquals(listOf("offline", "online"), calls)
    }

    @Test
    fun repeated_blocked_callbacks_report_once() {
        tracker.seed("100/1")
        tracker.setBlocked(false)
        tracker.setBlocked(true)
        tracker.setBlocked(true)
        assertEquals(listOf("offline"), calls)
    }

    @Test
    fun losing_a_blocked_network_does_not_leave_the_next_one_blocked() {
        tracker.seed("100/1")
        tracker.setBlocked(true)
        tracker.update(null)
        tracker.update("101/1")
        assertTrue(tracker.online)
        assertEquals(listOf("offline", "online"), calls)
    }

    @Test
    fun a_new_client_gets_the_state_read_at_launch() {
        tracker.seed("100/1")
        tracker.setBlocked(true)
        // AppModel.start() passes this straight to a client that defaults to online.
        assertFalse(tracker.online)
    }
}
