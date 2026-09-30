package sh.zeron.android.ui

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.MutableTransitionState
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.ime
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.TransformOrigin
import androidx.compose.ui.layout.positionInWindow
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalWindowInfo
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.IntRect
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.min
import androidx.compose.ui.window.Popup
import androidx.compose.ui.window.PopupPositionProvider
import androidx.compose.ui.window.PopupProperties

/** Side margin, anchor gap and the shortest popover worth opening above its chip. */
private val Margin = 12.dp
private val Gap = 8.dp
private val MinAbove = 260.dp

/**
 * A popover anchored to the composable it's declared in (a composer chip):
 * `wide` spans the window less its margins (capped for tablets), otherwise
 * it hugs its content. It opens above the chip when there's room — the
 * composer sits at the bottom — else below, and its height is bounded by
 * that space and [maxHeight], so it never covers the screen. Tapping
 * outside or Back dismisses it.
 */
@Composable
fun AnchoredPopover(
    expanded: Boolean,
    onDismiss: () -> Unit,
    wide: Boolean = true,
    maxHeight: Dp = 520.dp,
    content: @Composable ColumnScope.() -> Unit,
) {
    val density = LocalDensity.current
    var anchorTop by remember { mutableFloatStateOf(0f) }
    var anchorBottom by remember { mutableFloatStateOf(0f) }
    // Zero-size probe at the anchor's origin: where the chip sits in the window.
    Spacer(
        Modifier.size(0.dp).onGloballyPositioned {
            // positionInWindow: bounds of a zero-size node clip to nothing.
            anchorTop = it.positionInWindow().y
            anchorBottom = anchorTop + with(density) { 36.dp.toPx() }
        },
    )
    val visible = remember { MutableTransitionState(false) }
    visible.targetState = expanded
    if (!visible.currentState && !visible.targetState && visible.isIdle) return

    val window = LocalWindowInfo.current.containerSize
    val screenHeight = window.height.toFloat()
    val top = WindowInsets.statusBars.getTop(density).toFloat()
    val bottomInset = maxOf(WindowInsets.ime.getBottom(density), WindowInsets.navigationBars.getBottom(density)).toFloat()
    val margin = with(density) { (Margin + Gap).toPx() }
    val spaceAbove = with(density) { (anchorTop - top - margin).coerceAtLeast(0f).toDp() }
    val spaceBelow = with(density) { (screenHeight - bottomInset - anchorBottom - margin).coerceAtLeast(0f).toDp() }
    val above = spaceAbove >= MinAbove || spaceAbove >= spaceBelow
    val height = min(maxHeight, if (above) spaceAbove else spaceBelow)
    val width = min(with(density) { window.width.toDp() } - Margin * 2, 560.dp)

    val provider = remember(above, density) {
        AnchoredPosition(above, with(density) { Margin.roundToPx() }, with(density) { Gap.roundToPx() })
    }
    Popup(popupPositionProvider = provider, onDismissRequest = onDismiss, properties = PopupProperties(focusable = true)) {
        AnimatedVisibility(
            visibleState = visible,
            enter = fadeIn(MaterialTheme.motionScheme.fastEffectsSpec()) +
                scaleIn(MaterialTheme.motionScheme.fastSpatialSpec(), initialScale = 0.9f, transformOrigin = TransformOrigin(0.5f, if (above) 1f else 0f)),
            exit = fadeOut(MaterialTheme.motionScheme.fastEffectsSpec()) +
                scaleOut(MaterialTheme.motionScheme.fastSpatialSpec(), targetScale = 0.95f, transformOrigin = TransformOrigin(0.5f, if (above) 1f else 0f)),
        ) {
            Surface(
                shape = RoundedCornerShape(28.dp),
                color = MaterialTheme.colorScheme.surfaceContainerHigh,
                border = BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant),
                shadowElevation = 8.dp,
                modifier = Modifier.heightIn(max = height).then(if (wide) Modifier.width(width) else Modifier),
            ) {
                Column(content = content)
            }
        }
    }
}

/** Above (or below) the anchor, centred on it but kept inside the window's margins. */
private class AnchoredPosition(private val above: Boolean, private val margin: Int, private val gap: Int) : PopupPositionProvider {
    override fun calculatePosition(anchorBounds: IntRect, windowSize: IntSize, layoutDirection: LayoutDirection, popupContentSize: IntSize): IntOffset {
        val maxX = (windowSize.width - popupContentSize.width - margin).coerceAtLeast(margin)
        val x = (anchorBounds.center.x - popupContentSize.width / 2).coerceIn(margin, maxX)
        val y = if (above) anchorBounds.top - gap - popupContentSize.height else anchorBounds.bottom + gap
        return IntOffset(x, y.coerceIn(0, (windowSize.height - popupContentSize.height).coerceAtLeast(0)))
    }
}
