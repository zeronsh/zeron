package sh.zeron.android.ui

import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.geometry.Rect
import uniffi.zeron_core.SectionView

/** A long-pressed session row: its id (the menu re-reads the row when it opens) and bounds. */
data class RowMenuTarget(val id: String, val archived: Boolean, val anchor: Rect)

/**
 * Long-press menus for session rows and section headers. Rows live inside
 * lazy lists, so the menu itself is drawn once at screen level (a full-screen
 * AnchoredMenu inside an item would be clipped to that item).
 */
@Stable
class RowMenuHost {
    var row by mutableStateOf<RowMenuTarget?>(null)
    var header by mutableStateOf<Pair<SectionView, Rect>?>(null)
}

val LocalRowMenus = staticCompositionLocalOf { RowMenuHost() }
val LocalSwipe = staticCompositionLocalOf { SwipeCoordinator() }
