package sh.zeron.android.terminal

import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.Path
import android.graphics.RectF
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt

/**
 * Box drawing (U+2500–257F), block elements (U+2580–259F) and the `⏺`
 * bullet, drawn instead of shaped (iOS `TerminalGlyphs`). Geist Mono has none
 * of them, and a fallback font's glyphs neither fill the roomy line height
 * nor share the cell width: borders broke between rows and Claude Code's
 * block-art mascot fell apart. Painted from the cell's own rect, snapped to
 * pixels, so neighbors meet exactly.
 */
object TerminalGlyphs {
    fun draws(cp: Int): Boolean = cp in 0x2500..0x259F || cp == 0x23FA

    private val fill = Paint().apply { style = Paint.Style.FILL }
    private val stroke = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        style = Paint.Style.STROKE
        strokeCap = Paint.Cap.BUTT
    }
    private val dot = Paint(Paint.ANTI_ALIAS_FLAG).apply { style = Paint.Style.FILL }
    private val path = Path()

    private fun snap(v: Float) = v.roundToInt().toFloat()

    /** `cell` in pixels (y down); `baseline` and `xHeight` place the bullet like a text glyph. */
    fun draw(cp: Int, cell: RectF, baseline: Float, xHeight: Float, color: Int, canvas: Canvas) {
        val r = RectF(snap(cell.left), snap(cell.top), snap(cell.right), snap(cell.bottom))
        fill.color = color
        fill.alpha = color ushr 24
        when {
            cp == 0x23FA -> {
                dot.color = color
                val d = snap(min(r.width() * 0.62f, xHeight * 1.15f))
                canvas.drawOval(r.centerX() - d / 2, baseline - xHeight / 2 - d / 2, r.centerX() + d / 2, baseline - xHeight / 2 + d / 2, dot)
            }
            cp >= 0x2580 -> block(cp, r, canvas, color)
            else -> box(cp, r, canvas, color)
        }
    }

    private fun rect(canvas: Canvas, x0: Float, y0: Float, x1: Float, y1: Float) {
        canvas.drawRect(min(x0, x1), min(y0, y1), max(x0, x1), max(y0, y1), fill)
    }

    // ── Block elements ──────────────────────────────────────────────────

    private fun block(v: Int, r: RectF, canvas: Canvas, color: Int) {
        fun part(x0: Float, y0: Float, x1: Float, y1: Float) =
            rect(canvas, snap(r.left + r.width() * x0), snap(r.top + r.height() * y0), snap(r.left + r.width() * x1), snap(r.top + r.height() * y1))
        when (v) {
            0x2580 -> part(0f, 0f, 1f, 0.5f)
            in 0x2581..0x2588 -> part(0f, 1 - (v - 0x2580) / 8f, 1f, 1f)
            in 0x2589..0x258F -> part(0f, 0f, (0x2590 - v) / 8f, 1f)
            0x2590 -> part(0.5f, 0f, 1f, 1f)
            in 0x2591..0x2593 -> {
                fill.alpha = ((color ushr 24) * (v - 0x2590) / 4f).roundToInt()
                part(0f, 0f, 1f, 1f)
            }
            0x2594 -> part(0f, 0f, 1f, 1f / 8)
            0x2595 -> part(7f / 8, 0f, 1f, 1f)
            else -> {
                // Quadrants: upper left, upper right, lower left, lower right.
                val mask = when (v) {
                    0x2596 -> 4; 0x2597 -> 8; 0x2598 -> 1; 0x2599 -> 13; 0x259A -> 9
                    0x259B -> 7; 0x259C -> 11; 0x259D -> 2; 0x259E -> 6; 0x259F -> 14
                    else -> 0
                }
                if (mask and 1 != 0) part(0f, 0f, 0.5f, 0.5f)
                if (mask and 2 != 0) part(0.5f, 0f, 1f, 0.5f)
                if (mask and 4 != 0) part(0f, 0.5f, 0.5f, 1f)
                if (mask and 8 != 0) part(0.5f, 0.5f, 1f, 1f)
            }
        }
    }

    // ── Box drawing ─────────────────────────────────────────────────────

    /**
     * Per code point from U+2500: the left, up, right and down arms — 0 none,
     * 1 light, 2 heavy, 3 double. `.` marks the arcs and diagonals.
     */
    private val arms = arrayOf(
        "1010", "2020", "0101", "0202", "1010", "2020", "0101", "0202",
        "1010", "2020", "0101", "0202", "0011", "0021", "0012", "0022",
        "1001", "2001", "1002", "2002", "0110", "0120", "0210", "0220",
        "1100", "2100", "1200", "2200", "0111", "0121", "0211", "0112",
        "0212", "0221", "0122", "0222", "1101", "2101", "1201", "1102",
        "1202", "2201", "2102", "2202", "1011", "2011", "1021", "2021",
        "1012", "2012", "1022", "2022", "1110", "2110", "1120", "2120",
        "1210", "2210", "1220", "2220", "1111", "2111", "1121", "2121",
        "1211", "1112", "1212", "2211", "1221", "2112", "1122", "2221",
        "2122", "2212", "1222", "2222", "1010", "2020", "0101", "0202",
        "3030", "0303", "0031", "0013", "0033", "3001", "1003", "3003",
        "0130", "0310", "0330", "3100", "1300", "3300", "0131", "0313",
        "0333", "3101", "1303", "3303", "3031", "1013", "3033", "3130",
        "1310", "3330", "3131", "1313", "3333", ".", ".", ".",
        ".", ".", ".", ".", "1000", "0100", "0010", "0001",
        "2000", "0200", "0020", "0002", "1020", "0102", "2010", "0201",
    )

    private fun box(v: Int, r: RectF, canvas: Canvas, color: Int) {
        val light = max(1f, snap(r.width() / 9))
        val heavy = max(2f, snap(light * 2))
        val gap = max(1f, light)
        val cx = snap(r.centerX() - light / 2) + light / 2
        val cy = snap(r.centerY() - light / 2) + light / 2
        val code = arms[v - 0x2500]
        if (code == ".") {
            stroke.color = color
            stroke.strokeWidth = light
            path.reset()
            if (v in 0x256D..0x2570) {
                // Arcs: down-right, down-left, up-left, up-right.
                val right = v == 0x256D || v == 0x2570
                val down = v <= 0x256E
                val radius = min(r.width(), r.height()) / 2
                val endX = if (right) r.right else r.left
                val endY = if (down) r.bottom else r.top
                val sx = if (right) 1 else -1
                val sy = if (down) 1 else -1
                path.moveTo(cx, endY)
                path.lineTo(cx, cy + sy * radius)
                path.quadTo(cx, cy, cx + sx * radius, cy)
                path.lineTo(endX, cy)
            } else {
                if (v != 0x2572) {
                    path.moveTo(r.right, r.top)
                    path.lineTo(r.left, r.bottom)
                }
                if (v != 0x2571) {
                    path.moveTo(r.left, r.top)
                    path.lineTo(r.right, r.bottom)
                }
            }
            canvas.drawPath(path, stroke)
            return
        }
        val left = code[0] - '0'
        val up = code[1] - '0'
        val right = code[2] - '0'
        val down = code[3] - '0'
        fun width(style: Int) = if (style == 2) heavy else light
        // How far a straight arm reaches past the center to square off a join
        // with the perpendicular arms (half the thickest, or a double's outer line).
        fun reach(a: Int, b: Int): Float {
            if (a == 3 || b == 3) return gap + light / 2
            val style = max(a, b)
            return if (style == 0) 0f else width(style) / 2
        }
        val vReach = reach(up, down)
        val hReach = reach(left, right)
        fun horizontal(style: Int, end: Float) {
            if (style == 0) return
            val sign = if (end > cx) 1f else -1f
            if (style == 3) {
                for (side in floatArrayOf(-1f, 1f)) {
                    val near = if (side < 0) up else down
                    val far = if (side < 0) down else up
                    val start = when {
                        near != 0 -> if (near == 3) gap - light / 2 else light / 2
                        far != 0 -> -(if (far == 3) gap else 0f) - light / 2
                        else -> 0f
                    }
                    val y = cy + side * gap
                    rect(canvas, cx + sign * start, y - light / 2, end, y + light / 2)
                }
            } else {
                val t = width(style)
                rect(canvas, cx - sign * vReach, cy - t / 2, end, cy + t / 2)
            }
        }
        fun vertical(style: Int, end: Float) {
            if (style == 0) return
            val sign = if (end > cy) 1f else -1f
            if (style == 3) {
                for (side in floatArrayOf(-1f, 1f)) {
                    val near = if (side < 0) left else right
                    val far = if (side < 0) right else left
                    val start = when {
                        near != 0 -> if (near == 3) gap - light / 2 else light / 2
                        far != 0 -> -(if (far == 3) gap else 0f) - light / 2
                        else -> 0f
                    }
                    val x = cx + side * gap
                    rect(canvas, x - light / 2, cy + sign * start, x + light / 2, end)
                }
            } else {
                val t = width(style)
                rect(canvas, cx - t / 2, cy - sign * hReach, cx + t / 2, end)
            }
        }
        val dashes = when (v) {
            in 0x2504..0x2507 -> 3
            in 0x2508..0x250B -> 4
            in 0x254C..0x254F -> 2
            else -> 0
        }
        if (dashes > 0) {
            // Dashed lines: evenly spaced segments with a gap between.
            val horizontalLine = left != 0
            val t = width(max(left, up))
            val length = if (horizontalLine) r.width() else r.height()
            val step = length / dashes
            for (i in 0 until dashes) {
                val a = i * step + step * 0.15f
                val b = (i + 1) * step - step * 0.15f
                if (horizontalLine) {
                    rect(canvas, snap(r.left + a), cy - t / 2, snap(r.left + b), cy + t / 2)
                } else {
                    rect(canvas, cx - t / 2, snap(r.top + a), cx + t / 2, snap(r.top + b))
                }
            }
            return
        }
        horizontal(left, r.left)
        horizontal(right, r.right)
        vertical(up, r.top)
        vertical(down, r.bottom)
    }
}
