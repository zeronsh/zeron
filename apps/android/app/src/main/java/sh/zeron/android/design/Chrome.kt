package sh.zeron.android.design

import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.foundation.clickable
import android.content.Context
import android.graphics.Bitmap
import android.graphics.Canvas as AndroidCanvas
import android.graphics.Paint
import android.graphics.PorterDuff
import android.graphics.PorterDuffColorFilter
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.ui.geometry.Rect
import androidx.compose.foundation.layout.size
import androidx.compose.ui.Alignment
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathFillType
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.nativeCanvas
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.withTransform
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.layout.ContentScale
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import com.caverock.androidsvg.SVG
import kotlin.math.abs
import kotlin.math.roundToInt
import androidx.compose.ui.graphics.asAndroidPath

/**
 * A full-screen surface drawn *over* interactive content must eat taps in its
 * blank areas — `background` paints but handles no input, so an uncovered tap
 * falls through to whatever is underneath (a session row under the New
 * Session sheet opened that session). Consume the down event outright so
 * nothing below can start a press.
 */
fun Modifier.consumeBlankTaps(): Modifier =
    pointerInput(Unit) {
        awaitEachGesture {
            awaitFirstDown().consume()
        }
    }

@Composable
fun StatusMark(kind: MarkKind, colors: ZeronColors, modifier: Modifier = Modifier.size(12.dp)) {
    val spin = rememberInfiniteTransition(label = "glyph")
    val t by spin.animateFloat(
        initialValue = 0f,
        targetValue = 1f,
        animationSpec = infiniteRepeatable(tween(750, easing = LinearEasing)),
        label = "phase",
    )
    Canvas(modifier) {
        when (kind) {
            MarkKind.None -> Unit
            is MarkKind.Dot -> drawCircle(kind.color, radius = size.minDimension * 0.29f)
            is MarkKind.Check -> {
                val s = size.minDimension / 16f * 1.05f
                val ox = center.x - 8f * s
                val oy = center.y - 8f * s
                val p = Path().apply {
                    moveTo(ox + 3.5f * s, oy + 8.5f * s)
                    lineTo(ox + 6.5f * s, oy + 11.5f * s)
                    lineTo(ox + 12.5f * s, oy + 4.5f * s)
                }
                drawPath(p, kind.color, style = Stroke(width = 1.6f * (size.minDimension / 12f), cap = androidx.compose.ui.graphics.StrokeCap.Round, join = androidx.compose.ui.graphics.StrokeJoin.Round))
            }
            MarkKind.Spinner -> drawGrid(spinner = true, t = t, colors = colors)
            MarkKind.Trailer -> drawGrid(spinner = false, t = t, colors = colors)
        }
    }
}

private val trailerTints = listOf(Color(0xFFB6D3EF), Color(0xFFEDB185), Color(0xFFF888A0))
private val ring = arrayOf(intArrayOf(0, 1), intArrayOf(5, 2), intArrayOf(4, 3))

private fun androidx.compose.ui.graphics.drawscope.DrawScope.drawGrid(spinner: Boolean, t: Float, colors: ZeronColors) {
    val cols = if (spinner) 2 else 3
    val rows = if (spinner) 3 else 3
    val d = size.minDimension * (if (spinner) 3.5f else 3.5f) / 12f
    val gap = size.minDimension * (if (spinner) 1.5f else 1.75f) / 12f
    val gridW = d * cols + gap * (cols - 1)
    val gridH = d * rows + gap * (rows - 1)
    val ox = center.x - gridW / 2f
    val oy = center.y - gridH / 2f
    val tints = if (spinner) colors.glyph else trailerTints
    for (i in 0 until cols * rows) {
        val row = i / cols
        val col = i % cols
        val phase = if (spinner) ring[row][col] / 6f else (2 - row + abs(col - 1)) / 4f
        val alpha = glyphOpacity(t + phase).toFloat()
        drawCircle(
            color = tints[row].copy(alpha = alpha),
            radius = d / 2f,
            center = Offset(ox + col * (d + gap) + d / 2f, oy + row * (d + gap) + d / 2f),
        )
    }
}

/** `gspin_opacity`: hold bright, fall over 45%, rest dim, snap back over the last 8%. */
fun glyphOpacity(t: Float, dim: Float = 0.1f): Double {
    val u = t - kotlin.math.floor(t.toDouble()).toFloat()
    return when {
        u < 0.45f -> 1.0 + (dim - 1.0) * u / 0.45
        u < 0.92f -> dim.toDouble()
        else -> dim + (1.0 - dim) * (u - 0.92) / 0.08
    }
}

sealed interface MarkKind {
    data object None : MarkKind
    data object Spinner : MarkKind
    data object Trailer : MarkKind
    data class Dot(val color: Color) : MarkKind
    data class Check(val color: Color) : MarkKind
}

@Composable
fun ProjectTile(name: String, colorIndex: Int, colors: ZeronColors, size: Dp) {
    val tone = colors.project(colorIndex)
    val letter = name.trim().firstOrNull()?.uppercaseChar()?.toString() ?: "?"
    val context = LocalContext.current
    val face = remember { FontChain.face(context, uniffi.zeron_core.FaceRole.MONO_MEDIUM) }
    val ink = tone.copy(alpha = 0.85f)
    // 13pt desktop tile → radius 3, letter 9pt, scaled with the tile. The
    // letter is drawn, not laid out as Text: a Text line box (ascent +
    // descent, font padding) put the capital visibly low. Here its glyph box
    // is centred exactly, like iOS centring the capital rather than the line.
    Canvas(
        Modifier
            .size(size)
            .clip(RoundedCornerShape(size * 3f / 13f))
            .background(tone.copy(alpha = 0.08f)),
    ) {
        val paint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
            typeface = face
            textSize = this@Canvas.size.width * 9f / 13f
            color = android.graphics.Color.argb(
                (ink.alpha * 255).toInt(),
                (ink.red * 255).toInt(),
                (ink.green * 255).toInt(),
                (ink.blue * 255).toInt(),
            )
        }
        val glyph = android.graphics.Rect().also { paint.getTextBounds(letter, 0, letter.length, it) }
        val x = this.size.width / 2f - (glyph.left + glyph.right) / 2f
        val y = this.size.height / 2f - (glyph.top + glyph.bottom) / 2f
        drawContext.canvas.nativeCanvas.drawText(letter, x, y, paint)
    }
}

/**
 * Half the x-height of Geist Sans at [fontSize], in px: a project tile beside
 * a label sits centred on the label's x-height (iOS `xHeightCenter`).
 */
@Composable
fun rememberXHeightHalf(fontSize: androidx.compose.ui.unit.TextUnit): Float {
    val context = LocalContext.current
    val density = LocalDensity.current
    return remember(fontSize, density) {
        val paint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
            typeface = FontChain.face(context, uniffi.zeron_core.FaceRole.SANS)
            textSize = with(density) { fontSize.toPx() }
        }
        val r = android.graphics.Rect()
        paint.getTextBounds("x", 0, 1, r)
        r.height() / 2f
    }
}

/**
 * [tile] then [label], with the tile's centre on the label's x-height centre
 * (read from the label's first baseline), as the iOS session cell and
 * section header place it. Plain `CenterVertically` centred the tile on the
 * label's line box instead, which is not where the letters are.
 */
@Composable
fun TileBesideLabel(
    fontSize: androidx.compose.ui.unit.TextUnit,
    gap: Dp,
    tile: @Composable () -> Unit,
    modifier: Modifier = Modifier,
    label: @Composable () -> Unit,
) {
    val xHalf = rememberXHeightHalf(fontSize)
    androidx.compose.ui.layout.Layout(content = { tile(); label() }, modifier = modifier) { measurables, c ->
        val gapPx = gap.roundToPx()
        val t = measurables[0].measure(androidx.compose.ui.unit.Constraints())
        val labelMax = if (c.hasBoundedWidth) (c.maxWidth - t.width - gapPx).coerceAtLeast(0) else androidx.compose.ui.unit.Constraints.Infinity
        val l = measurables[1].measure(c.copy(minWidth = 0, maxWidth = labelMax, minHeight = 0))
        val baseline = l[androidx.compose.ui.layout.FirstBaseline].takeIf { it != androidx.compose.ui.layout.AlignmentLine.Unspecified }
            ?: (l.height * 0.78f).toInt()
        val tileY = (baseline - xHalf - t.height / 2f).roundToInt()
        val top = minOf(0, tileY)
        val height = maxOf(l.height, tileY + t.height) - top
        val width = maxOf(c.minWidth, t.width + gapPx + l.width)
        layout(width, maxOf(c.minHeight, height)) {
            val dy = (maxOf(c.minHeight, height) - height) / 2
            t.place(0, tileY - top + dy)
            l.place(t.width + gapPx, -top + dy)
        }
    }
}

/**
 * An agent's mark in a [size] square. With [fitInk] (the default) the
 * artwork's own ink bounds, not its viewBox, are fitted and centred, so
 * every mark fills the slot the same way: the viewBoxes pad unevenly (Pi's
 * π used 60% of its 800×800 box, Antigravity and Grok 93–97%, the rest the
 * full height), which made some marks small and others sit off-centre.
 */
@Composable
fun BrandMark(harness: String?, colors: ZeronColors, size: Dp, modifier: Modifier = Modifier, fitInk: Boolean = true) {
    val context = LocalContext.current
    val spec = remember(harness) { BrandLibrary.load(context, markFile(harness)) }
    val tint = colors.brandTint(harness)
    Canvas(modifier.size(size)) {
        val s = spec ?: return@Canvas
        // Aspect-fit and centre: the marks aren't square (Devin is 263×300,
        // Cursor 467×532, OpenCode 24×30), and scaling x and y separately
        // stretched them wide in the square slot.
        val box = if (fitInk) s.ink else Rect(0f, 0f, s.w, s.h)
        val k = minOf(this.size.width / box.width, this.size.height / box.height)
        val dx = (this.size.width - box.width * k) / 2f - box.left * k
        val dy = (this.size.height - box.height * k) / 2f - box.top * k
        withTransform({
            translate(dx, dy)
            scale(k, k, pivot = Offset.Zero)
        }) {
            drawPath(s.path, tint)
        }
    }
}

/**
 * Where a mark beside a title line should be centred, in px from the top
 * of the line's [lineHeight] box: halfway between the centre of the caps and
 * the centre of the x-height above the baseline. Titles are sentence case
 * (a capital, then mostly lowercase), so the line box centre, which sits
 * on the caps' centre, reads as too high next to the lowercase run, and
 * the x-height centre alone would drop the mark below the leading capital.
 */
@Composable
fun rememberTitleOpticalCenter(fontSize: androidx.compose.ui.unit.TextUnit, weight: androidx.compose.ui.text.font.FontWeight, lineHeight: Dp): Float {
    val context = LocalContext.current
    val density = LocalDensity.current
    val measurer = androidx.compose.ui.text.rememberTextMeasurer()
    return remember(fontSize, weight, lineHeight, density) {
        val style = androidx.compose.ui.text.TextStyle(fontFamily = ZeronType.Sans, fontWeight = weight, fontSize = fontSize)
        val layout = measurer.measure("Hx", style, maxLines = 1)
        val boxPx = with(density) { lineHeight.roundToPx() }
        // Row(verticalAlignment = CenterVertically) rounds the free space.
        val top = ((boxPx - layout.size.height) / 2f).roundToInt()
        val baseline = top + layout.firstBaseline
        val paint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
            typeface = FontChain.face(context, if (weight >= androidx.compose.ui.text.font.FontWeight.SemiBold) uniffi.zeron_core.FaceRole.SANS_SEMIBOLD else if (weight >= androidx.compose.ui.text.font.FontWeight.Medium) uniffi.zeron_core.FaceRole.SANS_MEDIUM else uniffi.zeron_core.FaceRole.SANS)
            textSize = with(density) { fontSize.toPx() }
        }
        val r = android.graphics.Rect()
        paint.getTextBounds("H", 0, 1, r)
        val cap = r.height().toFloat()
        paint.getTextBounds("x", 0, 1, r)
        val x = r.height().toFloat()
        baseline - (cap + x) / 4f
    }
}

/**
 * An agent mark beside a title line that starts [lineTop] into its parent
 * and is [lineHeight] tall: [mark] of ink in a [slot]-wide column, centred
 * on the title's optical centre ([rememberTitleOpticalCenter]). Place it
 * with `Alignment.Top` in the row.
 */
@Composable
fun TitleLineMark(
    harness: String?,
    colors: ZeronColors,
    fontSize: androidx.compose.ui.unit.TextUnit,
    weight: androidx.compose.ui.text.font.FontWeight,
    lineTop: Dp,
    lineHeight: Dp,
    modifier: Modifier = Modifier,
    slot: Dp = 20.dp,
    mark: Dp = 18.dp,
) {
    val density = LocalDensity.current
    val center = rememberTitleOpticalCenter(fontSize, weight, lineHeight)
    val top = with(density) { (lineTop.roundToPx() + center - mark.toPx() / 2f).toDp() }
    Box(modifier.width(slot).padding(top = top), contentAlignment = Alignment.TopCenter) {
        BrandMark(harness, colors, mark)
    }
}

/**
 * The tight bounds of [path]'s outline: points on the flattened curves,
 * not the Bézier control points that `Path.getBounds` also counts (which
 * padded Grok's and Antigravity's round shapes by up to 12%).
 */
private fun inkBounds(path: Path, tolerance: Float): Rect {
    val pts = path.asAndroidPath().approximate(tolerance)
    var l = Float.MAX_VALUE; var t = Float.MAX_VALUE; var r = -Float.MAX_VALUE; var b = -Float.MAX_VALUE
    var i = 0
    while (i + 2 < pts.size) {
        val x = pts[i + 1]; val y = pts[i + 2]
        if (x < l) l = x
        if (x > r) r = x
        if (y < t) t = y
        if (y > b) b = y
        i += 3
    }
    return if (l <= r && t <= b) Rect(l, t, r, b) else Rect.Zero
}

private data class BrandSpec(val w: Float, val h: Float, val path: Path, val ink: Rect)

private object BrandLibrary {
    private val cache = HashMap<String, BrandSpec?>()

    fun load(context: Context, name: String): BrandSpec? {
        cache[name]?.let { return it }
        val spec = runCatching {
            val text = context.assets.open("marks/$name.txt").bufferedReader().use { it.readText() }
            val nl = text.indexOf('\n')
            val header = text.substring(0, nl).split(" ")
            val w = header[0].toFloat()
            val h = header[1].toFloat()
            val even = header.getOrNull(2) == "1"
            val path = PathParser().parsePathString(text.substring(nl + 1).trim()).toPath().apply {
                fillType = if (even) PathFillType.EvenOdd else PathFillType.NonZero
            }
            // Ink bounds; a degenerate path falls back to the viewBox.
            val ink = inkBounds(path, maxOf(w, h) / 2000f).takeIf { it.width > 0f && it.height > 0f } ?: Rect(0f, 0f, w, h)
            BrandSpec(w, h, path, ink)
        }.getOrNull()
        cache[name] = spec
        return spec
    }
}

object IconAssets {
    private val cache = object : LinkedHashMap<String, Bitmap?>(64, 0.75f, true) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<String, Bitmap?>?) = size > 80
    }

    fun bitmap(context: Context, name: String, px: Int, tint: Int?): Bitmap? {
        if (px <= 0) return null
        val key = "$name@$px@$tint"
        if (cache.containsKey(key)) return cache[key]
        val bmp = runCatching {
            val svg = SVG.getFromAsset(context.assets, "icons/$name.svg") ?: return@runCatching null
            svg.setDocumentWidth(px.toFloat())
            svg.setDocumentHeight(px.toFloat())
            val raw = Bitmap.createBitmap(px, px, Bitmap.Config.ARGB_8888)
            svg.renderToCanvas(AndroidCanvas(raw))
            if (tint == null) raw else {
                val out = Bitmap.createBitmap(px, px, Bitmap.Config.ARGB_8888)
                val c = AndroidCanvas(out)
                val paint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
                    colorFilter = PorterDuffColorFilter(tint, PorterDuff.Mode.SRC_IN)
                }
                c.drawBitmap(raw, 0f, 0f, paint)
                out
            }
        }.getOrNull()
        cache[key] = bmp
        return bmp
    }
}

@Composable
fun AssetIcon(name: String, size: Dp, tint: Color? = null, modifier: Modifier = Modifier) {
    val context = LocalContext.current
    val px = with(LocalDensity.current) { size.roundToPx().coerceAtLeast(1) }
    val argb = tint?.let { android.graphics.Color.argb((it.alpha * 255).toInt(), (it.red * 255).toInt(), (it.green * 255).toInt(), (it.blue * 255).toInt()) }
    val bmp = remember(name, px, argb) { IconAssets.bitmap(context, name, px, argb) }
    if (bmp != null) {
        Image(
            bitmap = bmp.asImageBitmap(),
            contentDescription = null,
            modifier = modifier.size(size),
            contentScale = ContentScale.Fit,
        )
    } else {
        Box(modifier.size(size))
    }
}

@Composable
fun FolderPlusMark(color: Color, modifier: Modifier = Modifier) {
    Canvas(modifier) {
        val s = size.minDimension
        val stroke = Stroke(width = s * 0.08f, cap = androidx.compose.ui.graphics.StrokeCap.Round, join = androidx.compose.ui.graphics.StrokeJoin.Round)
        val p = Path().apply {
            moveTo(s * 0.08f, s * 0.32f)
            lineTo(s * 0.08f, s * 0.86f)
            lineTo(s * 0.62f, s * 0.86f)
            lineTo(s * 0.62f, s * 0.42f)
            lineTo(s * 0.40f, s * 0.42f)
            lineTo(s * 0.32f, s * 0.28f)
            lineTo(s * 0.08f, s * 0.28f)
            close()
        }
        drawPath(p, color, style = stroke)
        drawLine(color, Offset(s * 0.74f, s * 0.62f), Offset(s * 0.96f, s * 0.62f), stroke.width, androidx.compose.ui.graphics.StrokeCap.Round)
        drawLine(color, Offset(s * 0.85f, s * 0.51f), Offset(s * 0.85f, s * 0.73f), stroke.width, androidx.compose.ui.graphics.StrokeCap.Round)
    }
}

@Composable
fun PlusMark(color: Color, modifier: Modifier = Modifier) {
    Canvas(modifier) {
        val stroke = size.minDimension * 0.12f
        val inset = size.minDimension * 0.22f
        drawLine(color, Offset(center.x, inset), Offset(center.x, size.height - inset), stroke, androidx.compose.ui.graphics.StrokeCap.Round)
        drawLine(color, Offset(inset, center.y), Offset(size.width - inset, center.y), stroke, androidx.compose.ui.graphics.StrokeCap.Round)
    }
}

@Composable
fun ChevronMark(color: Color, modifier: Modifier = Modifier, expanded: Boolean = false) {
    Canvas(modifier) {
        val p = Path()
        if (expanded) {
            p.moveTo(size.width * 0.22f, size.height * 0.38f)
            p.lineTo(size.width * 0.5f, size.height * 0.66f)
            p.lineTo(size.width * 0.78f, size.height * 0.38f)
        } else {
            p.moveTo(size.width * 0.38f, size.height * 0.22f)
            p.lineTo(size.width * 0.66f, size.height * 0.5f)
            p.lineTo(size.width * 0.38f, size.height * 0.78f)
        }
        drawPath(p, color, style = Stroke(width = size.minDimension * 0.12f, cap = androidx.compose.ui.graphics.StrokeCap.Round, join = androidx.compose.ui.graphics.StrokeJoin.Round))
    }
}

@Composable
fun ArrowUpMark(color: Color, modifier: Modifier = Modifier) {
    Canvas(modifier) {
        val s = size.minDimension
        val p = Path().apply {
            moveTo(s * 0.5f, s * 0.22f)
            lineTo(s * 0.5f, s * 0.78f)
            moveTo(s * 0.28f, s * 0.46f)
            lineTo(s * 0.5f, s * 0.22f)
            lineTo(s * 0.72f, s * 0.46f)
        }
        drawPath(p, color, style = Stroke(width = s * 0.12f, cap = androidx.compose.ui.graphics.StrokeCap.Round, join = androidx.compose.ui.graphics.StrokeJoin.Round))
    }
}

@Composable
fun StopMark(color: Color, modifier: Modifier = Modifier) {
    Canvas(modifier) {
        val inset = size.minDimension * 0.30f
        drawRoundRect(color, topLeft = Offset(inset, inset), size = Size(size.width - inset * 2, size.height - inset * 2), cornerRadius = CornerRadius(size.minDimension * 0.08f))
    }
}

/**
 * The app's single back / close control: a 44dp glass circle with a chevron,
 * used at the leading edge of every pushed screen, sheet and editor (no text
 * "Close" / "Cancel" buttons). Announced as "Back" (localized).
 */
@Composable
fun BackButton(colors: ZeronColors, onClick: () -> Unit, modifier: Modifier = Modifier) {
    val label = androidx.compose.ui.res.stringResource(sh.zeron.android.R.string.nav_back)
    Box(
        modifier
            .size(44.dp)
            .glassSurface(colors, 22.dp)
            .clickable(onClickLabel = label, role = androidx.compose.ui.semantics.Role.Button, onClick = onClick)
            .semantics { contentDescription = label },
        contentAlignment = Alignment.Center,
    ) { BackChevron(colors.text, Modifier.size(18.dp)) }
}

@Composable
fun BackChevron(color: Color, modifier: Modifier = Modifier) {
    Canvas(modifier) {
        val p = Path()
        p.moveTo(size.width * 0.62f, size.height * 0.22f)
        p.lineTo(size.width * 0.34f, size.height * 0.5f)
        p.lineTo(size.width * 0.62f, size.height * 0.78f)
        drawPath(p, color, style = Stroke(width = size.minDimension * 0.11f, cap = androidx.compose.ui.graphics.StrokeCap.Round, join = androidx.compose.ui.graphics.StrokeJoin.Round))
    }
}

@Composable
fun ProfileMark(color: Color, modifier: Modifier = Modifier) {
    Canvas(modifier) {
        drawCircle(color.copy(alpha = 0.16f))
        drawCircle(color, radius = size.minDimension * 0.16f, center = Offset(center.x, center.y - size.minDimension * 0.10f))
        drawArc(
            color = color,
            startAngle = 200f,
            sweepAngle = 140f,
            useCenter = false,
            topLeft = Offset(size.width * 0.18f, size.height * 0.42f),
            size = Size(size.width * 0.64f, size.height * 0.64f),
            style = Stroke(width = size.minDimension * 0.08f, cap = androidx.compose.ui.graphics.StrokeCap.Round),
        )
    }
}

@Composable
fun EllipsisMark(color: Color, modifier: Modifier = Modifier) {
    Canvas(modifier) {
        val r = size.minDimension * 0.08f
        val y = center.y
        drawCircle(color, r, Offset(size.width * 0.22f, y))
        drawCircle(color, r, Offset(center.x, y))
        drawCircle(color, r, Offset(size.width * 0.78f, y))
    }
}

/** The reorder grip on pinned rows: three short bars, iOS-style. */
@Composable
fun ReorderMark(color: Color, modifier: Modifier = Modifier) {
    Canvas(modifier) {
        val s = size.minDimension
        val stroke = s * 0.09f
        val x0 = s * 0.24f
        val x1 = s * 0.76f
        for (i in 0..2) {
            val y = s * (0.30f + i * 0.20f)
            drawLine(color, Offset(x0, y), Offset(x1, y), stroke, androidx.compose.ui.graphics.StrokeCap.Round)
        }
    }
}

@Composable
fun PrGlyph(color: Color, modifier: Modifier = Modifier) {
    Canvas(modifier) { drawPrIcon(color) }
}

/** The desktop's pull-request icon tinted (iOS `PRIcon.image`), for the PR badge. */
@Composable
fun PullRequestIcon(tint: Color, size: Dp, modifier: Modifier = Modifier) {
    Canvas(modifier.size(size)) { drawPrIcon(tint) }
}

/** `pull-request.svg` on a 24-unit grid: three node circles, the bent connector and the arrowhead, one 1.5-unit stroke. */
private fun androidx.compose.ui.graphics.drawscope.DrawScope.drawPrIcon(color: Color) {
    val s = size.minDimension / 24f
    val stroke = Stroke(width = (1.5f * s * 1.15f).coerceAtLeast(1f), cap = androidx.compose.ui.graphics.StrokeCap.Round, join = androidx.compose.ui.graphics.StrokeJoin.Round)
    for (c in listOf(Offset(6f, 5f), Offset(6f, 19f), Offset(18f, 19f))) {
        drawCircle(color, radius = 2.25f * s, center = Offset(c.x * s, c.y * s), style = stroke)
    }
    val p = Path().apply {
        moveTo(6f * s, 7.25f * s); lineTo(6f * s, 16.75f * s)
        moveTo(15f * s, 5f * s); lineTo(15.75f * s, 5f * s)
        arcTo(androidx.compose.ui.geometry.Rect(13.5f * s, 5f * s, 18f * s, 9.5f * s), -90f, 90f, false)
        lineTo(18f * s, 16.75f * s)
        moveTo(12.75f * s, 7.75f * s); lineTo(15.25f * s, 5f * s); lineTo(12.75f * s, 2.25f * s)
    }
    drawPath(p, color, style = stroke)
}
