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

/** A long-pressed group header: Pinned (section == null) or a user section. */
data class HeaderMenuTarget(val id: String, val title: String, val section: SectionView?, val anchor: Rect)

/**
 * Long-press menus for session rows and group headers. Rows live inside
 * lazy lists, so the menu itself is drawn once at screen level (a full-screen
 * AnchoredMenu inside an item would be clipped to that item).
 */
@Stable
class RowMenuHost {
    var row by mutableStateOf<RowMenuTarget?>(null)
    var header by mutableStateOf<HeaderMenuTarget?>(null)
    /** The Move swipe action's "Move to Section" menu, anchored on the row. */
    var move by mutableStateOf<RowMenuTarget?>(null)
}

val LocalRowMenus = staticCompositionLocalOf { RowMenuHost() }
val LocalSwipe = staticCompositionLocalOf { SwipeCoordinator() }

/**
 * The right-hand column of the wide (foldable/tablet) shell. A session can
 * park a composable here — today the workspace file browser — and the shell
 * draws it beside the transcript instead of letting it cover the screen.
 * Null on the compact shell, where callers fall back to full-screen overlays.
 */
@Stable
class SidePanelState {
    var content by mutableStateOf<(@androidx.compose.runtime.Composable () -> Unit)?>(null)
}

val LocalSidePanel = staticCompositionLocalOf<SidePanelState?> { null }
