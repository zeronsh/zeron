package sh.zeron.android.design

import android.app.Activity
import android.content.Context
import android.graphics.Bitmap
import android.os.Handler
import android.os.Looper
import android.view.PixelCopy
import android.widget.FrameLayout
import androidx.compose.foundation.border
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.LayoutCoordinates
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInWindow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import java.io.File
import kotlin.math.max
import kotlin.math.min

/**
 * Root of the Compose hierarchy. Capsules sample a downscaled [PixelCopy] of
 * the window, box-blurred on the CPU. Recording the Compose tree into a
 * RenderNode and blurring it with RenderEffect crashes the emulator GPU, and
 * drawing the view into a software canvas from this process cannot write the
 * host `/tmp` (that threw and killed the activity). This path does neither.
 *
 * A sentinel in the app cache is written before the copy. If a previous
 * attempt died before the success file, later launches keep the flat fill.
 */
class GlassFrameLayout(context: Context) : FrameLayout(context) {
    private val main = Handler(Looper.getMainLooper())
    private val sentinel = File(context.cacheDir, "glass-blur-trying")
    private val okFile = File(context.cacheDir, "glass-blur-ok")
    private var scheduled = false
    private var copying = false

    override fun onAttachedToWindow() {
        super.onAttachedToWindow()
        val aborted = try {
            sentinel.exists() && !okFile.exists()
        } catch (_: Throwable) {
            false
        }
        if (aborted) disabled = true
    }

    override fun dispatchDraw(canvas: android.graphics.Canvas) {
        super.dispatchDraw(canvas)
        if (!disabled && !scheduled && !copying && width > 8 && height > 8) {
            scheduled = true
            main.postDelayed({ capture() }, 80)
        }
    }

    private fun capture() {
        scheduled = false
        if (disabled || copying || width < 8 || height < 8) return
        val window = (context as? Activity)?.window ?: run {
            disabled = true
            return
        }
        val scale = 8
        val bw = max(1, width / scale)
        val bh = max(1, height / scale)
        val bmp = Bitmap.createBitmap(bw, bh, Bitmap.Config.ARGB_8888)
        try {
            sentinel.writeText("1")
            if (okFile.exists()) okFile.delete()
        } catch (_: Throwable) {
        }
        copying = true
        try {
            PixelCopy.request(window, bmp, { result ->
                copying = false
                if (result == PixelCopy.SUCCESS) {
                    boxBlur(bmp, radius = 2)
                    backdrop = bmp
                    viewWidth = width
                    viewHeight = height
                    mode = "pixelcopy-cpu-boxblur"
                    try {
                        okFile.writeText("1")
                        sentinel.delete()
                    } catch (_: Throwable) {
                    }
                    postInvalidate()
                    scheduled = true
                    main.postDelayed({ capture() }, 280)
                } else {
                    disabled = true
                    mode = "pixelcopy-failed-$result"
                    try {
                        sentinel.delete()
                    } catch (_: Throwable) {
                    }
                }
            }, main)
        } catch (_: Throwable) {
            copying = false
            disabled = true
            mode = "pixelcopy-threw"
        }
    }

    companion object {
        @Volatile var disabled: Boolean = false
        @Volatile var backdrop: Bitmap? = null
        @Volatile var viewWidth: Int = 0
        @Volatile var viewHeight: Int = 0
        @Volatile var mode: String = "flat-fill"

        /** What the last capture attempt did, for the parity notes. */
        fun report(): String = when {
            backdrop != null -> "downscaled PixelCopy of the window, CPU box-blur, sampled behind the capsule"
            disabled -> "blur disabled ($mode); flat #1E1E1E fill kept"
            else -> "flat fill (snapshot not ready)"
        }
    }
}

val LocalGlassFrame = staticCompositionLocalOf<GlassFrameLayout?> { null }

/**
 * Frosted capsule. Prefers the CPU-blurred snapshot of the pixels behind the
 * capsule; otherwise the measured iOS glass tone (`#1E1E1E` on the demo backdrop).
 */
@Composable
fun Modifier.glassSurface(colors: ZeronColors, radius: Dp): Modifier {
    val shape = RoundedCornerShape(radius)
    val fill = if (colors.dark) Color(0xFF1E1E1E).copy(alpha = 0.72f) else Color.White.copy(alpha = 0.72f)
    val line = if (colors.dark) Color.White.copy(alpha = 0.16f) else Color.White.copy(alpha = 0.92f)
    var origin by remember { mutableStateOf(Offset.Zero) }
    return this
        .clip(shape)
        .onGloballyPositioned { coords: LayoutCoordinates ->
            val p = coords.positionInWindow()
            origin = Offset(p.x, p.y)
        }
        .drawWithContent {
            val bmp = GlassFrameLayout.backdrop
            val vw = GlassFrameLayout.viewWidth
            val vh = GlassFrameLayout.viewHeight
            if (bmp != null && vw > 0 && vh > 0) {
                val sx = (origin.x / vw * bmp.width).toInt().coerceIn(0, bmp.width - 1)
                val sy = (origin.y / vh * bmp.height).toInt().coerceIn(0, bmp.height - 1)
                val sw = (size.width / vw * bmp.width).toInt().coerceAtLeast(1).coerceAtMost(bmp.width - sx)
                val sh = (size.height / vh * bmp.height).toInt().coerceAtLeast(1).coerceAtMost(bmp.height - sy)
                drawImage(
                    bmp.asImageBitmap(),
                    srcOffset = androidx.compose.ui.unit.IntOffset(sx, sy),
                    srcSize = androidx.compose.ui.unit.IntSize(sw, sh),
                    dstSize = androidx.compose.ui.unit.IntSize(size.width.toInt().coerceAtLeast(1), size.height.toInt().coerceAtLeast(1)),
                )
                drawRect(fill)
            } else {
                drawRect(if (colors.dark) Color(0xFF1E1E1E).copy(alpha = 0.92f) else Color.White.copy(alpha = 0.78f))
            }
            drawContent()
        }
        .border(0.6.dp, line, shape)
}

private fun boxBlur(bitmap: Bitmap, radius: Int) {
    val w = bitmap.width
    val h = bitmap.height
    val pixels = IntArray(w * h)
    bitmap.getPixels(pixels, 0, w, 0, 0, w, h)
    val out = IntArray(pixels.size)
    val r = radius.coerceAtLeast(1)
    for (y in 0 until h) {
        for (x in 0 until w) {
            var a = 0; var rr = 0; var g = 0; var b = 0; var n = 0
            val y0 = max(0, y - r); val y1 = min(h - 1, y + r)
            val x0 = max(0, x - r); val x1 = min(w - 1, x + r)
            for (yy in y0..y1) {
                val row = yy * w
                for (xx in x0..x1) {
                    val p = pixels[row + xx]
                    a += p ushr 24
                    rr += (p shr 16) and 0xFF
                    g += (p shr 8) and 0xFF
                    b += p and 0xFF
                    n++
                }
            }
            out[y * w + x] = ((a / n) shl 24) or ((rr / n) shl 16) or ((g / n) shl 8) or (b / n)
        }
    }
    bitmap.setPixels(out, 0, w, 0, 0, w, h)
}
