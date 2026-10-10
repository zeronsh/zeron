package sh.zeron.android.transcript

import android.graphics.Canvas
import android.graphics.LinearGradient
import android.graphics.Paint
import android.graphics.PorterDuff
import android.graphics.PorterDuffXfermode
import android.graphics.RectF
import android.graphics.Shader
import sh.zeron.android.core.Fonts
import sh.zeron.android.design.TranscriptPalette
import uniffi.zeron_core.BoxStyle
import uniffi.zeron_core.Decoration
import uniffi.zeron_core.FadeEdge
import uniffi.zeron_core.RowDisplay
import uniffi.zeron_core.StyleDesc

/**
 * Style id → Paint for one transcript (ids are per layout view). Paints are
 * sized in pixels for the current density; main thread only (color mutates
 * per run while painting).
 */
class StyleFonts {
    private val paints = HashMap<Int, Paint>()
    private var density = 0f
    var count = 0
        private set

    fun update(styles: List<StyleDesc>, density: Float) {
        if (density != this.density) {
            paints.clear()
            this.density = density
        }
        for (s in styles) {
            val id = s.id.toInt()
            if (id !in paints) paints[id] = Fonts.paint(s.face, s.size * density, s.ligatures)
        }
        count = styles.size
    }

    operator fun get(id: UShort): Paint? = paints[id.toInt()]
}

/**
 * A display list plus everything derived from it once: runs, boxes and fades
 * grouped by layer (0 = the row canvas, n = scroller n-1).
 */
class RowModel(val display: RowDisplay) {
    val layers = display.scrollers.size + 1
    val runsByLayer: Array<IntArray>
    val boxesByLayer: Array<IntArray>
    /** Overflow fades per layer, each with the runs painted through it. */
    val fadesByLayer: Array<List<Pair<Int, IntArray>>>
    private val faded: BooleanArray

    init {
        val runs = Array(layers) { ArrayList<Int>() }
        val boxes = Array(layers) { ArrayList<Int>() }
        display.runs.forEachIndexed { i, r -> runs[r.scroller?.let { it.toInt() + 1 } ?: 0].add(i) }
        display.boxes.forEachIndexed { i, b -> boxes[b.scroller?.let { it.toInt() + 1 } ?: 0].add(i) }
        val fades = Array(layers) { ArrayList<Pair<Int, IntArray>>() }
        val masked = BooleanArray(display.runs.size)
        display.fades.forEachIndexed { fi, f ->
            val layer = f.scroller?.let { it.toInt() + 1 } ?: 0
            val under = runs[layer].filter { i ->
                val r = display.runs[i]
                !masked[i] && r.baseline > f.y && r.baseline <= f.y + f.h + 0.5f &&
                    (f.edge == FadeEdge.BOTTOM || r.x < f.x + f.w)
            }
            under.forEach { masked[it] = true }
            fades[layer].add(fi to under.toIntArray())
        }
        runsByLayer = Array(layers) { runs[it].toIntArray() }
        boxesByLayer = Array(layers) { boxes[it].toIntArray() }
        fadesByLayer = Array(layers) { fades[it] }
        faded = masked
    }

    /** Which runs to paint: `veilFrom` splits a streaming row into settled and fresh text. */
    sealed interface Pass {
        data object All : Pass
        /** Only text in `[from, to)` (UTF-16), clipped at glyph edges; no boxes or fades. */
        data class Range(val from: Int, val to: Int) : Pass
        /** Boxes and overflow fades only (the text is drawn by ranges). */
        data object Chrome : Pass
    }

    private val fill = Paint(Paint.ANTI_ALIAS_FLAG)
    private val stroke = Paint(Paint.ANTI_ALIAS_FLAG).apply { style = Paint.Style.STROKE }
    private val eraser = Paint().apply { xfermode = PorterDuffXfermode(PorterDuff.Mode.DST_OUT) }
    private val rect = RectF()

    /** Paint one layer in its own coordinates (dp × `d` = px). */
    fun draw(canvas: Canvas, layer: Int, d: Float, fonts: StyleFonts, palette: TranscriptPalette, pass: Pass = Pass.All) {
        val hairline = 1f
        if (pass !is Pass.Range) {
            for (i in boxesByLayer[layer]) {
                val b = display.boxes[i]
                rect.set(b.x * d, b.y * d, (b.x + b.w) * d, (b.y + b.h) * d)
                val r = b.radius * d
                when (b.style) {
                    BoxStyle.FILL -> {
                        fill.color = palette[b.color]
                        canvas.drawRoundRect(rect, r, r, fill)
                    }
                    BoxStyle.HAIRLINE -> {
                        stroke.color = palette[b.color]
                        stroke.strokeWidth = hairline
                        rect.inset(hairline / 2, hairline / 2)
                        val rr = maxOf(0f, r - hairline / 2)
                        canvas.drawRoundRect(rect, rr, rr, stroke)
                    }
                }
            }
        }
        for (i in runsByLayer[layer]) {
            if (faded[i]) continue
            val run = display.runs[i]
            val start = run.start.toInt()
            val end = start + run.len.toInt()
            val (from, to) = when (pass) {
                Pass.All -> 0 to Int.MAX_VALUE
                is Pass.Range -> pass.from to pass.to
                Pass.Chrome -> break
            }
            if (end <= from || start >= to) continue
            var clipped = false
            if (start < from || end > to) {
                val paint = fonts[run.style]
                fun dx(at: Int) = paint?.getRunAdvance(display.text, start, end, start, end, false, at) ?: 0f
                val left = if (start < from) run.x * d + dx(from) else -1e5f
                val right = if (end > to) run.x * d + dx(to) else 1e5f
                canvas.save()
                canvas.clipRect(left, -1e5f, right, 1e5f)
                clipped = true
            }
            drawRun(canvas, i, d, fonts, palette)
            if (clipped) canvas.restore()
        }
        if (pass !is Pass.Range) drawFades(canvas, layer, d, fonts, palette)
    }

    private fun drawRun(canvas: Canvas, i: Int, d: Float, fonts: StyleFonts, palette: TranscriptPalette, color: Int? = null) {
        val run = display.runs[i]
        val paint = fonts[run.style] ?: return
        val start = run.start.toInt()
        val end = start + run.len.toInt()
        if (end > display.text.length) return
        paint.color = color ?: palette[run.color]
        canvas.drawText(display.text, start, end, run.x * d, run.baseline * d, paint)
        if (run.decoration != Decoration.NONE) {
            val y = if (run.decoration == Decoration.UNDERLINE) run.baseline + 2f else run.baseline - 5f
            fill.color = paint.color
            canvas.drawRect(run.x * d, y * d, (run.x + run.width) * d, y * d + maxOf(1f, d * 0.5f), fill)
        }
    }

    /** Canvas-layer runs whose baseline sits inside the band, in one color (shimmer). */
    fun drawRunsIn(canvas: Canvas, x: Float, y: Float, w: Float, h: Float, color: Int, d: Float, fonts: StyleFonts, palette: TranscriptPalette) {
        for (i in runsByLayer[0]) {
            val r = display.runs[i]
            if (r.baseline <= y || r.baseline > y + h + 2 || r.x >= x + w) continue
            drawRun(canvas, i, d, fonts, palette, color)
        }
    }

    /** Runs under a fade are painted into a layer, then erased along a ramp. */
    private fun drawFades(canvas: Canvas, layer: Int, d: Float, fonts: StyleFonts, palette: TranscriptPalette) {
        for ((fi, runs) in fadesByLayer[layer]) {
            if (runs.isEmpty()) continue
            val f = display.fades[fi]
            val x0 = f.x * d
            val x1 = (f.x + f.w) * d
            val y0 = f.y * d
            val y1 = (f.y + f.h) * d
            val pad = 40 * d
            val save = canvas.save()
            if (f.edge == FadeEdge.TRAILING) canvas.clipRect(-1e5f, y0 - pad, x1, y1 + pad)
            canvas.saveLayer(-1e5f, y0 - pad * 2, 1e5f, y1 + pad * 2, null)
            for (i in runs) drawRun(canvas, i, d, fonts, palette)
            when (f.edge) {
                FadeEdge.TRAILING -> {
                    eraser.shader = LinearGradient(x0, 0f, x1, 0f, 0x00000000, 0xFF000000.toInt(), Shader.TileMode.CLAMP)
                    canvas.drawRect(x0, y0 - pad, x1, y1 + pad, eraser)
                }
                FadeEdge.BOTTOM -> {
                    eraser.shader = LinearGradient(0f, y0, 0f, y1, 0x00000000, 0xEB000000.toInt(), Shader.TileMode.CLAMP)
                    canvas.drawRect(x0 - pad, y0, x1 + pad, y1 + pad, eraser)
                }
            }
            canvas.restoreToCount(save)
        }
    }
}
