package sh.zeron.android.design

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
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.withTransform
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import com.caverock.androidsvg.SVG
import kotlin.math.abs

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

private val glyphTintsLight = listOf(Color(0xFF7965EC), Color(0xFF5B43E8), Color(0xFF4332AC))
private val glyphTintsDark = listOf(Color(0xFFABA1F9), Color(0xFF8B7CF6), Color(0xFF7266CA))
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
    val tints = if (spinner) (if (colors.dark) glyphTintsDark else glyphTintsLight) else trailerTints
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
    Box(
        modifier = Modifier
            .size(size)
            .clip(RoundedCornerShape(size * 3f / 13f))
            .background(tone.copy(alpha = 0.08f)),
        contentAlignment = androidx.compose.ui.Alignment.Center,
    ) {
        androidx.compose.material3.Text(
            text = letter,
            color = tone.copy(alpha = 0.85f),
            fontFamily = ZeronType.Mono,
            fontWeight = androidx.compose.ui.text.font.FontWeight.Medium,
            fontSize = with(LocalDensity.current) { (size * 9f / 13f).toSp() },
        )
    }
}

@Composable
fun BrandMark(harness: String?, colors: ZeronColors, size: Dp, modifier: Modifier = Modifier) {
    val context = LocalContext.current
    val spec = remember(harness) { BrandLibrary.load(context, markFile(harness)) }
    val tint = colors.brandTint(harness)
    Canvas(modifier.size(size)) {
        val s = spec ?: return@Canvas
        val path = s.path
        val sx = this.size.width / s.w
        val sy = this.size.height / s.h
        withTransform({ scale(sx, sy, pivot = Offset.Zero) }) {
            drawPath(path, tint)
        }
    }
}

private data class BrandSpec(val w: Float, val h: Float, val path: Path)

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
            BrandSpec(w, h, path)
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

@Composable
fun PrGlyph(color: Color, modifier: Modifier = Modifier) {
    Canvas(modifier) {
        val s = size.minDimension / 24f
        val stroke = Stroke(width = (1.5f * s * 1.15f).coerceAtLeast(1f), cap = androidx.compose.ui.graphics.StrokeCap.Round, join = androidx.compose.ui.graphics.StrokeJoin.Round)
        for (c in listOf(Offset(6f, 5f), Offset(6f, 19f), Offset(18f, 19f))) {
            drawCircle(color, radius = 2.25f * s, center = Offset(c.x * s, c.y * s), style = stroke)
        }
        val p = Path().apply {
            moveTo(6f * s, 7.25f * s); lineTo(6f * s, 16.75f * s)
            moveTo(15f * s, 5f * s); lineTo(15.75f * s, 5f * s)
            // quarter arc approximated
            lineTo(18f * s, 7.25f * s); lineTo(18f * s, 16.75f * s)
            moveTo(12.75f * s, 7.75f * s); lineTo(15.25f * s, 5f * s); lineTo(12.75f * s, 2.25f * s)
        }
        drawPath(p, color, style = stroke)
    }
}
