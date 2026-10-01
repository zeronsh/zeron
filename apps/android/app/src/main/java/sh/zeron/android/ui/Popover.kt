package sh.zeron.android.ui

import sh.zeron.android.feedback.ExpandedFeedback
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.MutableTransitionState
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.requiredSize
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.pointerInput
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
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
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

/** Space kept between a window with an overhang and the screen's start edge. */
private val OverhangEdge = 4.dp
private val MinAbove = 260.dp

/**
 * A popover anchored to the composable it's declared in (a composer chip):
 * `wide` spans the window less its margins (capped for tablets), otherwise
 * it hugs its content. It opens above the chip when there's room — the
 * composer sits at the bottom — else below, and its height is bounded by
 * that space and [maxHeight], so it never covers the screen. Tapping
 * outside or Back dismisses it. A fixed [width] (capped to the window) makes
 * a compact card; [scrim] dims the page behind and catches taps outside.
 *
 * [overlay] is drawn over the card, unclipped, in a layer the size of the card
 * plus [overhang] on its start side: something that straddles the card's edge
 * (the picker's provider rail) lives there, since the card clips its content.
 * The card keeps its place; the window just extends [overhang] further left.
 */
@Composable
fun AnchoredPopover(
    expanded: Boolean,
    onDismiss: () -> Unit,
    wide: Boolean = true,
    maxHeight: Dp = 520.dp,
    width: Dp? = null,
    scrim: Boolean = false,
    overhang: Dp = 0.dp,
    overlay: (@Composable BoxScope.() -> Unit)? = null,
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
    // Opening and closing are heard as they start, not when the animation ends.
    ExpandedFeedback(expanded)
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
    val windowWidth = with(density) { window.width.toDp() } - Margin * 2 - if (overhang > 0.dp) overhang + OverhangEdge - Margin else 0.dp
    val cardWidth = min(windowWidth, width ?: 560.dp)

    val provider = remember(above, density, width, overhang) {
        AnchoredPosition(
            above, with(density) { Margin.roundToPx() }, with(density) { Gap.roundToPx() }, startAligned = width != null,
            overhang = with(density) { overhang.roundToPx() }, edge = with(density) { OverhangEdge.roundToPx() },
        )
    }
    if (scrim) {
        // A full-window layer under the card: dims the page and takes taps outside it.
        Popup(popupPositionProvider = ScrimPosition, onDismissRequest = onDismiss, properties = PopupProperties(focusable = false, clippingEnabled = false)) {
            var entered by remember { androidx.compose.runtime.mutableStateOf(false) }
            androidx.compose.runtime.LaunchedEffect(Unit) { entered = true }
            val fade by androidx.compose.animation.core.animateFloatAsState(
                if (entered && visible.targetState) 1f else 0f,
                MaterialTheme.motionScheme.defaultEffectsSpec(),
                label = "scrim",
            )
            Box(
                Modifier
                    .requiredSize(with(density) { window.width.toDp() }, with(density) { window.height.toDp() })
                    .background(Color.Black.copy(alpha = 0.22f * fade))
                    .pointerInput(Unit) { detectTapGestures { onDismiss() } },
            )
        }
    }
    Popup(popupPositionProvider = provider, onDismissRequest = onDismiss, properties = PopupProperties(focusable = true)) {
        AnimatedVisibility(
            visibleState = visible,
            enter = fadeIn(MaterialTheme.motionScheme.fastEffectsSpec()) +
                scaleIn(MaterialTheme.motionScheme.fastSpatialSpec(), initialScale = 0.9f, transformOrigin = TransformOrigin(0.5f, if (above) 1f else 0f)),
            exit = fadeOut(MaterialTheme.motionScheme.fastEffectsSpec()) +
                scaleOut(MaterialTheme.motionScheme.fastSpatialSpec(), targetScale = 0.95f, transformOrigin = TransformOrigin(0.5f, if (above) 1f else 0f)),
        ) {
            Box {
                Surface(
                    shape = RoundedCornerShape(28.dp),
                    color = MaterialTheme.colorScheme.surfaceContainerHigh,
                    border = BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant),
                    shadowElevation = 8.dp,
                    modifier = Modifier.padding(start = overhang).heightIn(max = height).then(if (wide || width != null) Modifier.width(cardWidth) else Modifier),
                ) {
                    Column(content = content)
                }
                if (overlay != null) Box(Modifier.matchParentSize(), content = overlay)
            }
        }
    }
}

/** The window's top-left corner, whatever the anchor. */
private object ScrimPosition : PopupPositionProvider {
    override fun calculatePosition(anchorBounds: IntRect, windowSize: IntSize, layoutDirection: LayoutDirection, popupContentSize: IntSize) = IntOffset.Zero
}

/** Above (or below) the anchor, centred on it (or, for a fixed-width card, starting at its edge) but kept inside the window's margins. */
private class AnchoredPosition(
    private val above: Boolean,
    private val margin: Int,
    private val gap: Int,
    private val startAligned: Boolean = false,
    private val overhang: Int = 0,
    private val edge: Int = 0,
) : PopupPositionProvider {
    override fun calculatePosition(anchorBounds: IntRect, windowSize: IntSize, layoutDirection: LayoutDirection, popupContentSize: IntSize): IntOffset {
        // With an overhang the card sits `overhang` in from the window's start; keep the card where it would be.
        val minX = if (overhang > 0) edge else margin
        val maxX = (windowSize.width - popupContentSize.width - margin).coerceAtLeast(minX)
        val card = popupContentSize.width - overhang
        val x = (if (startAligned) anchorBounds.left - overhang else anchorBounds.center.x - card / 2 - overhang).coerceIn(minX, maxX)
        val y = if (above) anchorBounds.top - gap - popupContentSize.height else anchorBounds.bottom + gap
        return IntOffset(x, y.coerceIn(0, (windowSize.height - popupContentSize.height).coerceAtLeast(0)))
    }
}
