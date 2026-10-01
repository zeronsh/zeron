package sh.zeron.android.ui

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.spring
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.width
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.foundation.layout.offset
import kotlinx.coroutines.launch
import sh.zeron.android.design.ZeronType
import kotlin.math.abs
import kotlin.math.roundToInt

/** At most one row shows its swipe actions at a time (UITableView behavior). */
@Stable
class SwipeCoordinator {
    var openId by mutableStateOf<String?>(null)
}

/** A UIContextualAction look-alike: full-height colored button with an icon. */
class SwipeAction(
    val title: String,
    val color: Color,
    val icon: @Composable (Color) -> Unit,
    val onAction: () -> Unit,
)

/** Hooks the row content wires in: the moving offset, the drag detector, tap handling. */
class SwipeRowScope internal constructor(
    val offset: Modifier,
    val gesture: Modifier,
    val revealed: Boolean,
    private val close: () -> Boolean,
) {
    /** True when the tap only closed revealed actions (so don't open the row). */
    fun closeIfOpen(): Boolean = close()
}

private val ActionWidth = 84.dp

/**
 * iOS-style swipe actions. A short swipe reveals the button (tap it to act);
 * only a long, full swipe (past 60% of the row) acts on release. Mostly
 * vertical drags stay with the list ([detectRowSwipe]).
 */
@Composable
fun SwipeRow(
    id: String,
    height: Dp,
    coordinator: SwipeCoordinator,
    leading: SwipeAction?,
    trailing: SwipeAction?,
    modifier: Modifier = Modifier,
    content: @Composable BoxScope.(SwipeRowScope) -> Unit,
) {
    val scope = rememberCoroutineScope()
    val haptics = LocalHapticFeedback.current
    val actionPx = with(LocalDensity.current) { ActionWidth.toPx() }
    val offset = remember(id) { Animatable(0f) }
    var widthPx by remember { mutableStateOf(1f) }
    var base by remember { mutableStateOf(0f) }
    var dragging by remember { mutableStateOf(false) }
    var armed by remember { mutableStateOf(false) }
    val lead by rememberUpdatedState(leading)
    val trail by rememberUpdatedState(trailing)

    LaunchedEffect(coordinator.openId) {
        if (coordinator.openId != id && offset.value != 0f && !dragging) offset.animateTo(0f)
    }

    fun settle(target: Float) {
        if (target == 0f) {
            if (coordinator.openId == id) coordinator.openId = null
        } else {
            coordinator.openId = id
        }
        scope.launch { offset.animateTo(target, spring(stiffness = Spring.StiffnessMediumLow)) }
    }

    fun fire(action: SwipeAction) {
        if (coordinator.openId == id) coordinator.openId = null
        scope.launch { offset.animateTo(0f, spring(stiffness = Spring.StiffnessMediumLow)) }
        action.onAction()
    }

    val x = offset.value
    Box(modifier.fillMaxWidth().height(height).onSizeChanged { widthPx = it.width.toFloat().coerceAtLeast(1f) }.clipToBounds()) {
        val density = LocalDensity.current
        lead?.let { action ->
            if (x > 0.5f) {
                ActionPane(action, widthDp = with(density) { x.toDp() }, revealDp = with(density) { minOf(x, actionPx).toDp() }, leadingSide = true) { fire(action) }
            }
        }
        trail?.let { action ->
            if (x < -0.5f) {
                ActionPane(action, widthDp = with(density) { (-x).toDp() }, revealDp = with(density) { minOf(-x, actionPx).toDp() }, leadingSide = false) { fire(action) }
            }
        }
        val gesture = Modifier.pointerInput(id) {
            detectRowSwipe(
                onDrag = { total ->
                    if (!dragging) {
                        dragging = true
                        armed = false
                        base = offset.value
                        coordinator.openId = id
                    }
                    val min = if (trail != null) -widthPx else 0f
                    val max = if (lead != null) widthPx else 0f
                    val v = (base + total).coerceIn(min, max)
                    val full = abs(v) >= widthPx * 0.6f
                    if (full != armed) {
                        armed = full
                        if (full) haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                    }
                    scope.launch { offset.snapTo(v) }
                },
                onEnd = { total ->
                    dragging = false
                    val min = if (trail != null) -widthPx else 0f
                    val max = if (lead != null) widthPx else 0f
                    val v = (base + total).coerceIn(min, max)
                    val full = widthPx * 0.6f
                    val l = lead
                    val t = trail
                    when {
                        l != null && v >= full -> fire(l)
                        t != null && v <= -full -> fire(t)
                        l != null && v > actionPx / 2f -> settle(actionPx)
                        t != null && v < -actionPx / 2f -> settle(-actionPx)
                        else -> settle(0f)
                    }
                },
                onCancel = {
                    dragging = false
                    settle(0f)
                },
            )
        }
        val rowScope = SwipeRowScope(
            offset = Modifier.offset { IntOffset(offset.value.roundToInt(), 0) },
            gesture = gesture,
            revealed = abs(x) > 1f,
            close = {
                if (offset.value != 0f) {
                    settle(0f)
                    true
                } else {
                    false
                }
            },
        )
        content(rowScope)
    }
}

@Composable
private fun BoxScope.ActionPane(action: SwipeAction, widthDp: Dp, revealDp: Dp, leadingSide: Boolean, onClick: () -> Unit) {
    Box(
        Modifier
            .align(if (leadingSide) Alignment.CenterStart else Alignment.CenterEnd)
            .width(widthDp)
            .fillMaxHeight()
            .background(action.color)
            .clickable(onClick = onClick),
        // The label rides the row's edge, like UIContextualAction.
        contentAlignment = if (leadingSide) Alignment.CenterEnd else Alignment.CenterStart,
    ) {
        Column(
            Modifier.width(revealDp),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.Center,
        ) {
            if (revealDp > 36.dp) {
                action.icon(Color.White)
                Spacer(Modifier.height(4.dp))
                Text(action.title, color = Color.White, fontFamily = ZeronType.Sans, fontWeight = FontWeight.Medium, fontSize = 13.sp, maxLines = 1)
            }
        }
    }
}
