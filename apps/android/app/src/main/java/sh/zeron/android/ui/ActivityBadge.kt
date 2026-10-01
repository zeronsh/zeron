package sh.zeron.android.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.AllInclusive
import androidx.compose.material3.MaterialShapes
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawWithCache
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Matrix
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.asAndroidPath
import androidx.compose.ui.graphics.asComposePath
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.VectorGroup
import androidx.compose.ui.graphics.vector.VectorPath
import androidx.compose.ui.graphics.vector.toPath
import androidx.compose.ui.layout.Layout
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.graphics.shapes.RoundedPolygon
import sh.zeron.android.core.Fonts
import sh.zeron.android.core.SessionActivity
import sh.zeron.android.design.HarnessMark
import sh.zeron.android.design.LocalDarkTheme
import uniffi.zeron_core.FaceRole

@Composable
fun sessionActivityColor(shape: SessionActivity.Shape): Color = when (shape) {
    SessionActivity.Shape.MainRunning -> if (LocalDarkTheme.current) Color(0xFFAB9AFF) else Color(0xFF5B43E8)
    SessionActivity.Shape.SubagentsRunning -> if (LocalDarkTheme.current) Color(0xFFFACC45) else Color(0xFFB88700)
    SessionActivity.Shape.CallbackWaiting -> if (LocalDarkTheme.current) Color(0xFF8AB4FF) else Color(0xFF2967D8)
}

/** The harness tile of a session row. It is, and stays, this size: the count badge is an overlay and adds none. */
val HarnessTileSize = 48.dp
private val HarnessTileCorner = 16.dp

/** Every count badge occupies this square, whatever its shape or label. */
val CountBadgeSize = 24.dp

/**
 * How far in from the tile's corner the badge's centre sits: on the diagonal through the rounded corner's arc,
 * so the badge reads as pinned to the visible corner and overhangs the tile by about 40% of its size, up and right.
 */
val BadgeCornerInset = 4.dp

/** Which colours a count badge wears. */
enum class CountBadgeTone {
    /** Purple with white digits: a session row's tile. */
    Count,

    /** The subagents-running yellow with dark digits: the chat header's Subagents button. */
    Activity,
}

@Composable
private fun countBadgeColors(tone: CountBadgeTone): Pair<Color, Color> {
    val dark = LocalDarkTheme.current
    return when (tone) {
        CountBadgeTone.Count -> (if (dark) Color(0xFF7C61DB) else Color(0xFF5B43E8)) to Color.White
        CountBadgeTone.Activity -> sessionActivityColor(SessionActivity.Shape.SubagentsRunning) to Color(0xFF1F1700)
    }
}

// ── the art: Material polygon -> outline -> label fit ─────────────────────

private fun polygonOf(shape: BadgeShape): RoundedPolygon = when (shape) {
    BadgeShape.Pill -> MaterialShapes.Pill
    BadgeShape.Arch -> MaterialShapes.Arch
    BadgeShape.Triangle -> MaterialShapes.Triangle
    BadgeShape.Diamond -> MaterialShapes.Diamond
    BadgeShape.Pentagon -> MaterialShapes.Pentagon
    BadgeShape.Gem -> MaterialShapes.Gem
    BadgeShape.Cookie7Sided -> MaterialShapes.Cookie7Sided
    BadgeShape.Clover8Leaf -> MaterialShapes.Clover8Leaf
    BadgeShape.PuffyDiamond -> MaterialShapes.PuffyDiamond
    BadgeShape.ClamShell -> MaterialShapes.ClamShell
    BadgeShape.Puffy -> MaterialShapes.Puffy
    BadgeShape.Heart -> MaterialShapes.Heart
}

/** A Material polygon fitted into the unit square without distortion, as a drawable path and a sampled outline. */
internal class ShapeArt(val path: Path, val outline: Outline)

private const val SAMPLES_PER_CUBIC = 20

internal fun RoundedPolygon.art(scale: Float = 1f): ShapeArt {
    val raw = ArrayList<Pair<Float, Float>>()
    for (c in cubics) {
        for (i in 0 until SAMPLES_PER_CUBIC) {
            val t = i / SAMPLES_PER_CUBIC.toFloat(); val u = 1 - t
            val x = u * u * u * c.anchor0X + 3 * u * u * t * c.control0X + 3 * u * t * t * c.control1X + t * t * t * c.anchor1X
            val y = u * u * u * c.anchor0Y + 3 * u * u * t * c.control0Y + 3 * u * t * t * c.control1Y + t * t * t * c.anchor1Y
            raw += x to y
        }
    }
    val sampled = Outline(FloatArray(raw.size) { raw[it].first }, FloatArray(raw.size) { raw[it].second })
    val n = sampled.normalization()
    // [scale] shrinks the shape about the footprint's centre (the label keeps its size, so it sits snugger).
    fun sx(v: Float) = 0.5f + (n.x(v) - 0.5f) * scale
    fun sy(v: Float) = 0.5f + (n.y(v) - 0.5f) * scale
    val path = Path()
    cubics.forEachIndexed { i, c ->
        if (i == 0) path.moveTo(sx(c.anchor0X), sy(c.anchor0Y))
        path.cubicTo(sx(c.control0X), sy(c.control0Y), sx(c.control1X), sy(c.control1Y), sx(c.anchor1X), sy(c.anchor1Y))
    }
    path.close()
    val unit = sampled.normalized()
    return ShapeArt(path, Outline(FloatArray(unit.size) { 0.5f + (unit.xs[it] - 0.5f) * scale }, FloatArray(unit.size) { 0.5f + (unit.ys[it] - 0.5f) * scale }))
}

/** The ink of the labels, measured from the face itself (Geist Bold) and never from a line box. */
internal object LabelInk {
    private const val REF = 100f
    private val paint by lazy {
        android.graphics.Paint(android.graphics.Paint.ANTI_ALIAS_FLAG).apply {
            typeface = Fonts.typeface(FaceRole.SANS_BOLD)
            textSize = REF
            isLinearText = true
        }
    }

    /** A label's outline at [REF] px with its baseline at y = 0, and that outline's horizontal ink extent. */
    class Ink(val path: android.graphics.Path, val left: Float, val right: Float)

    private val cache = HashMap<String, Ink>()

    @Synchronized
    fun of(label: String): Ink = cache.getOrPut(label) {
        val p = android.graphics.Path()
        paint.getTextPath(label, 0, label.length, 0f, 0f, p)
        val b = android.graphics.RectF()
        p.computeBounds(b, true)
        Ink(p, b.left, b.right)
    }

    /** The digits' height: the flat-topped "1" has no overshoot. */
    val digitHeight: Float by lazy {
        val b = android.graphics.RectF()
        of("1").path.computeBounds(b, true)
        b.height()
    }

    /** The widest ink width / digit height over every label of [digits] digits: what the fit must make room for. */
    fun aspect(digits: Int): Float = aspects.getOrPut(digits) {
        val labels = if (digits == 1) (0..9).map { "$it" } else (10..99).map { "$it" }
        labels.maxOf { of(it).let { ink -> ink.right - ink.left } } / digitHeight
    }

    private val aspects = HashMap<Int, Float>()
}

/** The overflow icon's outline, in its own 24-unit viewport. */
internal object OverflowInk {
    val path: Path by lazy { Icons.Rounded.AllInclusive.outline() }
    val bounds by lazy { path.getBounds() }

    private fun ImageVector.outline(): Path {
        val out = Path()
        fun walk(node: androidx.compose.ui.graphics.vector.VectorNode) {
            when (node) {
                is VectorPath -> out.addPath(node.pathData.toPath())
                is VectorGroup -> node.forEach { walk(it) }
            }
        }
        walk(root)
        return out
    }
}

/** Everything the badge draws for one count, in unit-square coordinates; computed once per shape and label. */
internal class BadgePlan(val art: ShapeArt, val fit: LabelFit, val label: String?)

private val arts = HashMap<BadgeShape, ShapeArt>()
private val fits = HashMap<Pair<BadgeShape, Int>, LabelFit>()

@Synchronized
internal fun badgePlan(count: UInt): BadgePlan {
    val shape = BadgeShape.of(count)
    val art = arts.getOrPut(shape) { polygonOf(shape).art(BadgeGeometry.shapeScale(shape)) }
    val label = SessionActivity.badgeLabel(count)
    val key = shape to (label?.length ?: 0)
    val fit = fits.getOrPut(key) {
        val aspect = if (label == null) OverflowInk.bounds.width / OverflowInk.bounds.height else LabelInk.aspect(label.length)
        // Digits share one height per digit count in every shape; the overflow icon takes the room the shape has.
        BadgeGeometry.fit(art.outline, aspect, height = label?.let { BadgeGeometry.digitHeight(it.length) }, clearance = if (label == null) BadgeGeometry.ICON_CLEARANCE else BadgeGeometry.CLEARANCE)
    }
    return BadgePlan(art, fit, label)
}

/**
 * A running-subagent count: a Material shape scaled uniformly into a fixed [size] square, the number at the shape's optical
 * centre. Drawn on a canvas in dp, so it ignores the system font size entirely, and with its digits placed by their real
 * ink (a flat digit box centred vertically, the label's ink centred horizontally), not by a text line box.
 */
@Composable
fun SubagentCountBadge(
    count: UInt,
    modifier: Modifier = Modifier,
    size: Dp = CountBadgeSize,
    tone: CountBadgeTone = CountBadgeTone.Count,
    describe: Boolean = true,
) {
    if (count == 0u) return
    val plan = remember(count) { badgePlan(count) }
    val (fill, ink) = countBadgeColors(tone)
    Box(
        modifier
            .size(size)
            .clearAndSetSemantics {
                // The overflow icon is visual shorthand; announce the real count.
                if (describe) contentDescription = if (count == 1u) "1 subagent running" else "$count subagents running"
            }
            .drawWithCache {
                val px = this.size.width
                val scale = Matrix().apply { scale(px, px) }
                val shapePath = Path().apply { addPath(plan.art.path); transform(scale) }
                val labelPath = labelPath(plan, px)
                onDrawBehind {
                    drawPath(shapePath, fill)
                    drawPath(labelPath, ink)
                }
            },
    )
}

/** The label of [plan] positioned in a [px]-square footprint: digit height = fit height, ink centred on the fit centre. */
private fun labelPath(plan: BadgePlan, px: Float): Path {
    val fit = plan.fit
    val cx = fit.cx * px
    val cy = fit.cy * px
    val label = plan.label
    if (label == null) {
        val b = OverflowInk.bounds
        val k = fit.height * px / b.height
        val m = Matrix().apply {
            translate(cx - k * (b.left + b.right) / 2, cy - k * (b.top + b.bottom) / 2)
            scale(k, k)
        }
        return Path().apply { addPath(OverflowInk.path); transform(m) }
    }
    val ink = LabelInk.of(label)
    val k = fit.height * px / LabelInk.digitHeight
    // The baseline sits half a digit below the centre; the digit box is [baseline - digitHeight, baseline].
    val baseline = cy + fit.height * px / 2
    val m = android.graphics.Matrix().apply {
        setScale(k, k)
        postTranslate(cx - k * (ink.left + ink.right) / 2, baseline)
    }
    val out = android.graphics.Path()
    ink.path.transform(m, out)
    return out.asComposePath()
}

/**
 * [content] with [badge] pinned over its top-right corner as a pure overlay: this layout is exactly [content]'s size, so the
 * badge never moves or resizes anything. The badge's centre sits [inset] inside the corner; the rest of it spills out of
 * the layout's bounds, which is the point (a parent that clips would cut it, so give it room, not a bigger layout).
 */
@Composable
fun CornerBadge(inset: Dp, badge: (@Composable () -> Unit)?, modifier: Modifier = Modifier, content: @Composable () -> Unit) {
    Layout(
        modifier = modifier,
        content = {
            Box { content() }
            if (badge != null) Box { badge() }
        },
    ) { measurables, constraints ->
        val main = measurables[0].measure(constraints)
        val over = measurables.getOrNull(1)?.measure(Constraints())
        layout(main.width, main.height) {
            main.place(0, 0)
            val edge = inset.roundToPx()
            over?.place(main.width - edge - over.width / 2, edge - over.height / 2)
        }
    }
}

/**
 * The leading tile of a session row. Its layout is exactly the harness tile and nothing else ([HarnessTileSize]
 * square, whether or not subagents run); the count badge is an overlay pinned to the tile's top-right corner, overhanging it
 * and contributing nothing to the row's measurements.
 */
@Composable
fun HarnessActivityTile(harness: String?, tone: Color, running: UInt, modifier: Modifier = Modifier) {
    CornerBadge(BadgeCornerInset, if (running > 0u) ({ SubagentCountBadge(running) }) else null, modifier) {
        Box(
            Modifier.size(HarnessTileSize).clip(RoundedCornerShape(HarnessTileCorner)).background(tone.copy(alpha = if (LocalDarkTheme.current) .18f else .12f)),
            contentAlignment = Alignment.Center,
        ) { HarnessMark(harness, 24.dp, tint = MaterialTheme.colorScheme.onSurface) }
    }
}
