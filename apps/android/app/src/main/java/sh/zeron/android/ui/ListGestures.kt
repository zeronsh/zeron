package sh.zeron.android.ui

import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.PointerInputScope
import androidx.compose.ui.input.pointer.changedToUpIgnoreConsumed
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.positionChangeIgnoreConsumed
import kotlin.math.abs
import kotlin.math.hypot

/**
 * Horizontal swipe on a list row that never steals a vertical scroll.
 *
 * `detectHorizontalDragGestures` claims the gesture as soon as the finger
 * drifts sideways past the touch slop, even when the drag is mostly vertical,
 * so a slightly diagonal scroll froze the list and slid the row instead
 * ("sticky" scrolling). Here the row only takes over when the movement is
 * clearly horizontal: well past the slop and at least twice the vertical
 * travel. Anything else is left to the list. Once claimed, the row consumes
 * the drag, so the row's tap handler cancels and nothing opens by accident.
 */
internal suspend fun PointerInputScope.detectRowSwipe(
    onDrag: (total: Float) -> Unit,
    onEnd: (total: Float) -> Unit,
    onCancel: () -> Unit,
) {
    awaitEachGesture {
        val down = awaitFirstDown(requireUnconsumed = false)
        val slop = viewConfiguration.touchSlop
        var dx = 0f
        var dy = 0f
        var claimed = false
        while (true) {
            val event = awaitPointerEvent()
            val change = event.changes.firstOrNull { it.id == down.id } ?: break
            if (change.changedToUpIgnoreConsumed() || !change.pressed) {
                if (claimed) {
                    change.consume()
                    onEnd(dx)
                }
                return@awaitEachGesture
            }
            val delta = change.positionChangeIgnoreConsumed()
            if (!claimed) {
                // The list (or another handler) already took this gesture.
                if (change.isConsumed) break
                dx += delta.x
                dy += delta.y
                when {
                    abs(dy) > slop && abs(dy) >= abs(dx) -> break
                    abs(dx) > slop * 1.5f && abs(dx) > abs(dy) * 2f -> {
                        claimed = true
                        change.consume()
                        onDrag(dx)
                    }
                    hypot(dx, dy) > slop * 3f -> break
                }
            } else {
                dx += delta.x
                change.consume()
                onDrag(dx)
            }
        }
        if (claimed) onCancel()
    }
}

/**
 * Decides whether a finished touch on the list may count as a tap.
 *
 * Two cases are not taps even though Compose's clickable would accept them:
 * - the list was still moving when the finger came down: touching a flinging
 *   list should only stop it (as on iOS and native Android lists);
 * - the finger ended up further than the touch slop from where it started.
 *   Compose only cancels a click on movement it saw as a separate move event,
 *   so when move events arrive late or merged (a busy main thread, or the
 *   emulator's injected swipes) a real drag was taken for a tap.
 */
internal class TapGuard {
    var downWhileScrolling = false
    var movedAway = false

    fun allowsTap(): Boolean = !downWhileScrolling && !movedAway
}

internal fun Modifier.tapGuard(guard: TapGuard, isScrolling: () -> Boolean): Modifier = pointerInput(guard) {
    awaitEachGesture {
        val down = awaitFirstDown(requireUnconsumed = false, pass = PointerEventPass.Initial)
        guard.downWhileScrolling = isScrolling()
        guard.movedAway = false
        val slop = viewConfiguration.touchSlop
        while (true) {
            // Initial pass: runs before the row's click handler sees the same event.
            val event = awaitPointerEvent(PointerEventPass.Initial)
            val change = event.changes.firstOrNull { it.id == down.id } ?: break
            if ((change.position - down.position).getDistance() > slop) guard.movedAway = true
            if (!change.pressed) break
        }
    }
}
