package sh.zeron.android.ui

import org.junit.Assert.assertEquals
import org.junit.Test

class GroupRowsTest {
    private val nine = (1..9).toList()

    @Test fun longGroupShowsItsFirstFiveAndOffersTheRest() {
        assertEquals(GroupRows(listOf(1, 2, 3, 4, 5), "Show all 9"), groupRows(nine, collapsed = false, cappable = true, expanded = false))
    }

    @Test fun expandedGroupShowsEveryRowAndOffersShowLess() {
        assertEquals(GroupRows(nine, "Show less"), groupRows(nine, collapsed = false, cappable = true, expanded = true))
    }

    @Test fun groupOfFiveOrFewerHasNoToggle() {
        val five = (1..5).toList()
        assertEquals(GroupRows(five, null), groupRows(five, collapsed = false, cappable = true, expanded = false))
        assertEquals(GroupRows(five, null), groupRows(five, collapsed = false, cappable = true, expanded = true))
        assertEquals(GroupRows(emptyList<Int>(), null), groupRows(emptyList<Int>(), collapsed = false, cappable = true, expanded = false))
    }

    @Test fun sixthRowIsWhereTheCapStarts() {
        val six = (1..6).toList()
        assertEquals(GroupRows(six.take(GROUP_PREVIEW), "Show all 6"), groupRows(six, collapsed = false, cappable = true, expanded = false))
    }

    @Test fun collapsedGroupShowsNothing() {
        assertEquals(GroupRows(emptyList<Int>(), null), groupRows(nine, collapsed = true, cappable = true, expanded = false))
        assertEquals(GroupRows(emptyList<Int>(), null), groupRows(nine, collapsed = true, cappable = true, expanded = true))
    }

    // The filter tabs (Needs you, Working, Pinned) list their rows without a cap.
    @Test fun uncappableGroupShowsEveryRow() {
        assertEquals(GroupRows(nine, null), groupRows(nine, collapsed = false, cappable = false, expanded = false))
    }
}
