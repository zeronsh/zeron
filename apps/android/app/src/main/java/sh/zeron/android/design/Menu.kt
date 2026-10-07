package sh.zeron.android.design

import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.draw.clip
import androidx.compose.foundation.shape.RoundedCornerShape
import sh.zeron.android.R
import androidx.compose.ui.res.stringResource
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.Color
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.statusBars
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.layout.layout
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/** One row of an iOS-style pull-down menu (UIMenu / UIAction). */
data class MenuEntry(
    val title: String,
    val subtitle: String? = null,
    val checked: Boolean = false,
    val destructive: Boolean = false,
    val icon: (@Composable (Color) -> Unit)? = null,
    /**
     * An inline section start (UIMenu .displayInline): divider + small title.
     * With an [icon] the icon sits at the header's trailing edge as a small
     * button that runs [onClick] without closing the menu.
     */
    val header: Boolean = false,
    /** Runs [onClick] without closing the menu (drill-down rows, toggles). */
    val keepOpen: Boolean = false,
    /** A drill-down row: trailing chevron, opens a nested list in place (keeps the menu open). */
    val submenu: Boolean = false,
    /** The nested list's "up" row: leading back chevron + the parent's name. */
    val back: Boolean = false,
    val onClick: () -> Unit,
)

/** Starts an inline group; a blank title draws just the divider. */
fun menuSection(title: String = "") = MenuEntry(title, header = true) {}

/**
 * A UIMenu look-alike: glass panel, optional small title, rows with a leading
 * checkmark column when any row carries state. [loading] shows a quiet
 * placeholder while a deferred element (models, efforts) resolves.
 */
@Composable
fun MenuPanel(
    colors: ZeronColors,
    title: String?,
    entries: List<MenuEntry>,
    modifier: Modifier = Modifier,
    loading: Boolean = false,
    onDismiss: () -> Unit,
) {
    val stateful = entries.any { it.checked && !it.header }
    Column(
        modifier
            .widthIn(min = 230.dp, max = 300.dp)
            .glassSurface(colors, 22.dp)
            // UIMenu's material is a heavy blur; without a live blur, a denser
            // wash keeps rows legible over busy transcript text.
            .background(if (colors.dark) Color(0xFF232325).copy(alpha = 0.86f) else Color(0xFFF7F7F8).copy(alpha = 0.86f))
            .heightIn(max = 460.dp)
            .verticalScroll(rememberScrollState())
            .padding(vertical = 6.dp),
    ) {
        if (!title.isNullOrBlank()) {
            Text(
                title,
                color = colors.secondary,
                fontFamily = ZeronType.Sans,
                fontSize = 13.sp,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.padding(start = 16.dp, end = 16.dp, top = 6.dp, bottom = 6.dp),
            )
            MenuDivider(colors, Modifier.padding(bottom = 2.dp))
        }
        if (loading && entries.isEmpty()) {
            Text(stringResource(R.string.loading), color = colors.tertiary, fontFamily = ZeronType.Sans, fontSize = 16.sp, modifier = Modifier.padding(horizontal = 16.dp, vertical = 12.dp))
        }
        entries.forEachIndexed { index, entry ->
            if (entry.header) {
                // A section is set off by the same hairline as the title (the
                // title's own line already separates a leading section).
                if (index > 0) MenuDivider(colors, Modifier.padding(vertical = 4.dp))
                if (entry.title.isNotBlank() || entry.icon != null) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(
                            entry.title,
                            color = colors.secondary,
                            fontFamily = ZeronType.Sans,
                            fontSize = 13.sp,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                            modifier = Modifier.weight(1f, fill = false).padding(start = 16.dp, end = 4.dp, top = 6.dp, bottom = 2.dp),
                        )
                        entry.icon?.let { icon ->
                            Box(
                                Modifier.size(32.dp).clip(RoundedCornerShape(8.dp)).clickable(onClick = entry.onClick),
                                contentAlignment = Alignment.Center,
                            ) { icon(colors.secondary) }
                        }
                    }
                }
                return@forEachIndexed
            }
            if (entry.back) {
                Row(
                    Modifier
                        .fillMaxWidth()
                        .heightIn(min = 40.dp)
                        .clickable(onClick = entry.onClick)
                        .padding(horizontal = 10.dp, vertical = 6.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    BackChevron(colors.accent, Modifier.size(18.dp))
                    Spacer(Modifier.width(4.dp))
                    Text(entry.title, color = colors.accent, fontFamily = ZeronType.Sans, fontWeight = FontWeight.Medium, fontSize = 15.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
                MenuDivider(colors, Modifier.padding(bottom = 2.dp))
                return@forEachIndexed
            }
            val tint = if (entry.destructive) colors.danger else colors.text
            Row(
                Modifier
                    .fillMaxWidth()
                    .heightIn(min = 44.dp)
                    .clickable {
                        if (!entry.keepOpen && !entry.submenu) onDismiss()
                        entry.onClick()
                    }
                    .padding(horizontal = 14.dp, vertical = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                if (stateful) {
                    Box(Modifier.width(24.dp), contentAlignment = Alignment.CenterStart) {
                        if (entry.checked) CheckGlyph(tint, Modifier.size(14.dp))
                    }
                }
                Column(Modifier.weight(1f)) {
                    Text(entry.title, color = tint, fontFamily = ZeronType.Sans, fontSize = 16.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    entry.subtitle?.takeIf { it.isNotBlank() }?.let {
                        Text(it, color = colors.secondary, fontFamily = ZeronType.Sans, fontSize = 13.sp, maxLines = 2, overflow = TextOverflow.Ellipsis)
                    }
                }
                entry.icon?.let {
                    Spacer(Modifier.width(10.dp))
                    it(tint)
                }
                if (entry.submenu) {
                    Spacer(Modifier.width(8.dp))
                    BackChevron(colors.tertiary, Modifier.size(14.dp).graphicsLayer { rotationZ = 180f })
                }
            }
        }
    }
}

/**
 * The one separator for menus, popups and sheets: a thin hairline, never a
 * thick group bar.
 */
@Composable
fun MenuDivider(colors: ZeronColors, modifier: Modifier = Modifier) {
    // iOS separator tone: `hairline` alone vanishes on the dark glass panel.
    HorizontalDivider(color = colors.text.copy(alpha = if (colors.dark) 0.14f else 0.12f), thickness = 0.5.dp, modifier = modifier)
}

/**
 * Full-screen, undimmed tap catcher (iOS menus don't dim the screen) with the
 * panel opening above [anchor] (root px), leading edges aligned, clamped to
 * the window.
 */
@Composable
fun AnchoredMenu(
    colors: ZeronColors,
    anchor: Rect,
    title: String?,
    entries: List<MenuEntry>,
    loading: Boolean = false,
    above: Boolean = true,
    onDismiss: () -> Unit,
) {
    val density = LocalDensity.current
    val gap = with(density) { 8.dp.roundToPx() }
    val margin = with(density) { 12.dp.roundToPx() }
    val top = WindowInsets.statusBars.getTop(density) + with(density) { 8.dp.roundToPx() }
    val none = remember { MutableInteractionSource() }
    // Android's Back closes the menu first (iOS has no Back; tapping outside
    // does the same there).
    BackHandler(onBack = onDismiss)
    BoxWithConstraints(
        Modifier
            .fillMaxSize()
            .clickable(interactionSource = none, indication = null, onClick = onDismiss),
    ) {
        MenuPanel(
            colors = colors,
            title = title,
            entries = entries,
            loading = loading,
            onDismiss = onDismiss,
            modifier = Modifier.layout { measurable, constraints ->
                // Above the anchor the panel gets the space down from the status
                // bar and scrolls past that, like UIMenu.
                val room = if (above) anchor.top.toInt() - gap - top else constraints.maxHeight - anchor.bottom.toInt() - gap - margin
                val placeable = measurable.measure(Constraints(maxWidth = constraints.maxWidth, maxHeight = room.coerceIn(1, constraints.maxHeight)))
                layout(constraints.maxWidth, constraints.maxHeight) {
                    val x = anchor.left.toInt().coerceIn(margin, (constraints.maxWidth - placeable.width - margin).coerceAtLeast(margin))
                    val y = if (above) anchor.top.toInt() - gap - placeable.height else anchor.bottom.toInt() + gap
                    placeable.place(x, y.coerceIn(top, (constraints.maxHeight - placeable.height - margin).coerceAtLeast(top)))
                }
            },
        )
    }
}

@Composable
fun CheckGlyph(color: Color, modifier: Modifier = Modifier) {
    Canvas(modifier) {
        val s = size.minDimension
        val path = androidx.compose.ui.graphics.Path().apply {
            moveTo(s * 0.12f, s * 0.55f)
            lineTo(s * 0.4f, s * 0.82f)
            lineTo(s * 0.9f, s * 0.2f)
        }
        drawPath(path, color, style = Stroke(width = s * 0.13f, cap = StrokeCap.Round, join = androidx.compose.ui.graphics.StrokeJoin.Round))
    }
}

/** SF Symbols' gauge.with.dots.needle.67percent, drawn. */
@Composable
fun GaugeGlyph(color: Color, modifier: Modifier = Modifier) {
    Canvas(modifier) {
        val s = size.minDimension
        val c = Offset(s / 2f, s * 0.58f)
        val r = s * 0.42f
        drawArc(color, 160f, 220f, false, topLeft = Offset(c.x - r, c.y - r), size = androidx.compose.ui.geometry.Size(r * 2, r * 2), style = Stroke(width = s * 0.1f, cap = StrokeCap.Round))
        val angle = Math.toRadians(-30.0)
        drawLine(color, c, Offset(c.x + (r * 0.7f * kotlin.math.cos(angle)).toFloat(), c.y + (r * 0.7f * kotlin.math.sin(angle)).toFloat()), strokeWidth = s * 0.1f, cap = StrokeCap.Round)
        drawCircle(color, s * 0.08f, c)
    }
}

/**
 * [AnchoredMenu] for anchors nested inside layouts that can't host a
 * full-screen overlay (the composer's send button): drawn in a popup window
 * spanning the screen. [anchor] is in window coordinates. Not focusable, so
 * an open keyboard stays up.
 */
@Composable
fun PopupAnchoredMenu(
    colors: ZeronColors,
    anchor: Rect,
    title: String?,
    entries: List<MenuEntry>,
    above: Boolean = true,
    onDismiss: () -> Unit,
) {
    androidx.compose.ui.window.Popup(
        popupPositionProvider = object : androidx.compose.ui.window.PopupPositionProvider {
            override fun calculatePosition(
                anchorBounds: androidx.compose.ui.unit.IntRect,
                windowSize: androidx.compose.ui.unit.IntSize,
                layoutDirection: androidx.compose.ui.unit.LayoutDirection,
                popupContentSize: androidx.compose.ui.unit.IntSize,
            ) = androidx.compose.ui.unit.IntOffset.Zero
        },
        onDismissRequest = onDismiss,
        properties = androidx.compose.ui.window.PopupProperties(focusable = false, clippingEnabled = false),
    ) {
        AnchoredMenu(colors, anchor, title, entries, above = above, onDismiss = onDismiss)
    }
}
