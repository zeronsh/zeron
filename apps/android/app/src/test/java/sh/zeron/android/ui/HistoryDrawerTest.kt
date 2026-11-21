package sh.zeron.android.ui

import androidx.compose.runtime.MonotonicFrameClock
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.yield
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.zeron_core.ChatIndicator
import uniffi.zeron_core.ProjectRef

private const val SLOP = 16f

private fun claim(dx: Float, dy: Float, drawer: Boolean = true, home: Boolean = true) = claimSwipe(dx, dy, SLOP, drawer, home)

private fun project(id: String, name: String) = ProjectRef(id, name, 0u)

/** A frame every time an animation asks, 16 ms apart. */
private class TestFrameClock : MonotonicFrameClock {
    private var nanos = 0L
    override suspend fun <R> withFrameNanos(onFrame: (Long) -> R): R {
        yield()
        nanos += 16_000_000L
        return onFrame(nanos)
    }
}

class HistoryDrawerTest {
    @Test fun waitsOutTheTouchSlop() {
        assertEquals(SwipeClaim.Undecided, claim(0f, 0f))
        assertEquals(SwipeClaim.Undecided, claim(SLOP, 0f))
        assertEquals(SwipeClaim.Undecided, claim(-SLOP, 2f))
    }

    @Test fun rightwardOpensTheDrawerLeftwardGoesHome() {
        assertEquals(SwipeClaim.Drawer, claim(20f, 0f))
        assertEquals(SwipeClaim.Home, claim(-20f, 0f))
    }

    @Test fun verticalDragsBelongToTheScroller() {
        assertEquals(SwipeClaim.Yield, claim(0f, 20f))
        assertEquals(SwipeClaim.Yield, claim(5f, -20f))
        // A dead-even diagonal goes to the scroller too.
        assertEquals(SwipeClaim.Yield, claim(20f, 20f))
    }

    @Test fun horizontalHasToWinClearly() {
        // Past the slop, but not 1.2x the vertical: keep watching.
        assertEquals(SwipeClaim.Undecided, claim(20f, 17f))
        assertEquals(SwipeClaim.Undecided, claim(-20f, 17f))
        assertEquals(SwipeClaim.Drawer, claim(30f, 17f))
    }

    @Test fun aSideWithNothingToOpenIsLeftAlone() {
        // Home has nowhere to swipe home to; a large screen has no drawer.
        assertEquals(SwipeClaim.Yield, claim(-20f, 0f, home = false))
        assertEquals(SwipeClaim.Yield, claim(20f, 0f, drawer = false))
        assertEquals(SwipeClaim.Drawer, claim(20f, 0f, home = false))
    }

    @Test fun drawerSettlesOnTheNearerSideWithoutAFling() {
        assertTrue(drawerSettlesOpen(0.6f, 0f))
        assertFalse(drawerSettlesOpen(0.4f, 0f))
        assertFalse(drawerSettlesOpen(0.5f, 0f))
        // Slow drifts don't count as flings.
        assertFalse(drawerSettlesOpen(0.4f, 500f))
        assertTrue(drawerSettlesOpen(0.6f, -500f))
    }

    @Test fun aFlingDecidesTheDrawerWhateverTheDistance() {
        assertTrue(drawerSettlesOpen(0.1f, 800f))
        assertFalse(drawerSettlesOpen(0.9f, -800f))
    }

    @Test fun pageGoesHomeOnceFortyPercentGone() {
        assertTrue(homeSwipeLeaves(0.45f, 0f))
        assertFalse(homeSwipeLeaves(0.35f, 0f))
    }

    @Test fun leftwardFlingSendsThePageHomeRightwardSpringsItBack() {
        // Velocity is the finger's: leftward is negative.
        assertTrue(homeSwipeLeaves(0.1f, -800f))
        assertFalse(homeSwipeLeaves(0.9f, 800f))
    }

    @Test fun projectFilterKeepsOnlyThatProject() {
        assertTrue(drawerShows(null, Filter.All, "a", ChatIndicator.IDLE, false))
        assertTrue(drawerShows(null, Filter.All, null, ChatIndicator.IDLE, false))
        assertTrue(drawerShows("a", Filter.All, "a", ChatIndicator.IDLE, false))
        assertFalse(drawerShows("a", Filter.All, "b", ChatIndicator.IDLE, false))
        // Projectless chats aren't in any project.
        assertFalse(drawerShows("a", Filter.All, null, ChatIndicator.IDLE, false))
    }

    @Test fun statusFiltersMatchTheirChats() {
        assertTrue(drawerShows(null, Filter.NeedsYou, null, ChatIndicator.AWAITING_INPUT, false))
        assertFalse(drawerShows(null, Filter.NeedsYou, null, ChatIndicator.WORKING, false))
        assertTrue(drawerShows(null, Filter.Working, null, ChatIndicator.WORKING, false))
        assertFalse(drawerShows(null, Filter.Working, null, ChatIndicator.ERRORED, false))
        assertTrue(drawerShows(null, Filter.Pinned, null, ChatIndicator.IDLE, true))
        assertFalse(drawerShows(null, Filter.Pinned, null, ChatIndicator.AWAITING_INPUT, false))
    }

    @Test fun bothFiltersApply() {
        assertTrue(drawerShows("a", Filter.Working, "a", ChatIndicator.WORKING, false))
        assertFalse(drawerShows("a", Filter.Working, "b", ChatIndicator.WORKING, false))
        assertFalse(drawerShows("a", Filter.Working, "a", ChatIndicator.IDLE, false))
    }

    @Test fun onlySameNamedCheckoutsShowTheirHost() {
        val projects = listOf(project("p1", "zeron"), project("p2", "zeron"), project("p3", "docs"))
        val hosts = mapOf("p1" to "laptop", "p2" to "hetzner-01", "p3" to "laptop")
        assertEquals(mapOf("p1" to "laptop", "p2" to "hetzner-01"), sameNameHosts(projects) { hosts[it] })
    }

    @Test fun aCheckoutWithNoKnownHostStaysBare() {
        val projects = listOf(project("p1", "zeron"), project("p2", "zeron"))
        assertEquals(mapOf("p2" to "hetzner-01"), sameNameHosts(projects) { if (it == "p2") "hetzner-01" else null })
        assertEquals(emptyMap<String, String>(), sameNameHosts(emptyList()) { "laptop" })
    }

    @Test fun aSwipedPageFinishesItsSlideFromWhereItWasLetGo() {
        assertEquals(-600, homeSwipeExitOffset(1000, 0.4f))
        assertEquals(-1000, homeSwipeExitOffset(1000, 0f))
        assertEquals(0, homeSwipeExitOffset(1000, 1f))
    }

    @Test fun onlyThePageThatTookTheSwipeIsPoppedByIt() {
        val home = HomeSwipeState()
        home.begin("chat-1")
        // Not moved yet: a back press now is a plain back.
        assertFalse(home.leaving("chat-1"))
        home.dragBy(0.3f)
        assertTrue(home.leaving("chat-1"))
        assertFalse(home.leaving("chat-2"))
    }

    @Test fun aNewSwipeCarriesOnFromAPageSpringingBack() {
        val home = HomeSwipeState()
        home.begin("chat-1")
        home.dragBy(0.3f)
        home.begin("chat-1")
        assertEquals(0.3f, home.progress)
        // Another page starts from rest.
        home.begin("chat-2")
        assertEquals(0f, home.progress)
    }

    @Test fun aFilterOnAProjectWithNoChatsLeftShowsAllProjects() {
        val projects = listOf(project("p1", "zeron"), project("p2", "docs"))
        assertEquals("p2", drawerProject("p2", projects))
        assertNull(drawerProject("gone", projects))
        assertNull(drawerProject("p1", emptyList()))
        assertNull(drawerProject(null, projects))
    }

    @Test fun theKeyboardStaysUntilTheDrawerIsAllTheWayOpen() {
        assertFalse(drawerTakesKeyboard(0f))
        assertFalse(drawerTakesKeyboard(0.05f))
        assertFalse(drawerTakesKeyboard(0.99f))
        assertTrue(drawerTakesKeyboard(1f))
    }

    @Test fun theDrawerOpensOnlyOverHomeChatsAndNewSessionOnPhones() {
        for (route in listOf(Routes.HOME, Routes.CHAT, Routes.NEW)) assertTrue(route, historyDrawerOn(route, wide = false))
        for (route in listOf(Routes.SEARCH, Routes.SETTINGS, Routes.VOICE, Routes.TERMINAL, null)) assertFalse("$route", historyDrawerOn(route, wide = false))
        assertFalse(historyDrawerOn(Routes.CHAT, wide = true))
    }

    @Test fun onlyChatsAndNewSessionSwipeHome() {
        assertTrue(swipesHome(Routes.CHAT, wide = false))
        assertTrue(swipesHome(Routes.NEW, wide = false))
        assertFalse(swipesHome(Routes.HOME, wide = false))
        assertFalse(swipesHome(Routes.CHAT, wide = true))
    }

    @Test fun dismissingShutsTheDrawerEvenMidAnimation() = runBlocking(TestFrameClock()) {
        val drawer = HistoryDrawerState()
        val opening = launch { drawer.open() }
        repeat(4) { yield() }
        assertTrue(drawer.progress > 0f && drawer.progress < 1f)
        drawer.dismiss()
        repeat(8) { yield() }
        assertEquals(0f, drawer.progress)
        assertTrue(opening.isCancelled)
    }
}
