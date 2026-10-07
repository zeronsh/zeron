package sh.zeron.android.ui

import android.content.Context
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.LinearGradient
import android.graphics.Paint
import android.graphics.Path
import android.graphics.PorterDuff
import android.graphics.PorterDuffXfermode
import android.graphics.RectF
import android.graphics.Shader
import android.graphics.Typeface
import android.os.SystemClock
import android.view.GestureDetector
import android.view.MotionEvent
import android.view.VelocityTracker
import android.view.View
import android.view.ViewConfiguration
import android.widget.OverScroller
import sh.zeron.android.design.FontChain
import sh.zeron.android.design.ZeronColors
import androidx.compose.ui.graphics.toArgb
import sh.zeron.android.design.glyphOpacity
import uniffi.zeron_core.BoxStyle
import uniffi.zeron_core.ColorRole
import uniffi.zeron_core.Decoration
import uniffi.zeron_core.FadeEdge
import uniffi.zeron_core.FaceRole
import uniffi.zeron_core.LayoutFrame
import uniffi.zeron_core.RowDisplay
import uniffi.zeron_core.StyleDesc
import uniffi.zeron_core.TranscriptView
import uniffi.zeron_core.Widget
import uniffi.zeron_core.WidgetKind
import kotlin.math.abs
import kotlin.math.hypot
import kotlin.math.max
import kotlin.math.min
import kotlin.math.sign

/**
 * Virtualized transcript. Rust publishes a [LayoutFrame]; this view paints the
 * visible rows at those coordinates with the same Geist bytes the core measured.
 */
class TranscriptListView(context: Context) : View(context) {
    companion object {
        /** How long a code block's copy button shows its check. */
        const val COPIED_MS = 2_000L
    }

    var engine: TranscriptView? = null
    var colors: ZeronColors? = null
    var faces: Map<FaceRole, Typeface> = emptyMap()
    var onToggle: (ULong) -> Unit = {}
    var onToggleDetail: (ULong, ULong, Boolean) -> Unit = { _, _, _ -> }
    var onCopy: (String) -> Unit = {}
    var onLink: (String) -> Unit = {}
    var onImage: (Bitmap) -> Unit = {}
    var onDetail: (String, String) -> Unit = { _, _ -> }
    /** Any tap on the transcript (iOS puts the composer/keyboard away). */
    var onTap: () -> Unit = {}
    var imageFor: (String) -> Bitmap? = { null }
    var requestImage: (String) -> Unit = {}
    /** A thumbnail was tapped before its bytes arrived (still gray). */
    var onImageMiss: () -> Unit = {}
    var onDistanceFromBottom: (Float) -> Unit = {}
    /** The user's own messages in transcript order, re-sent when they change (the message navigator). */
    var onUserMarks: (List<UserMark>) -> Unit = {}
    /** Index into the last [onUserMarks] list of the message being read (the last one at or above the top). */
    var onActiveUserMark: (Int) -> Unit = {}
    var savedScroll: Float = 0f
    var onScroll: (Float) -> Unit = {}
    /** A drag or fling started (true) or ended (false). */
    var onScrollActive: (Boolean) -> Unit = {}
    /**
     * Distance from the view's bottom edge to where messages stop: composer,
     * keyboard, and the gap above the composer. The last row rests just above
     * it; rows scrolled past it fade out over [bottomFadePx] and are hidden
     * below (they never show through the composer). Changing it while at the
     * bottom (the keyboard opening) keeps the latest message in view.
     */
    var bottomInsetPx: Int = 0
        set(value) {
            if (field == value) return
            field = value
            reclamp()
        }
    /** Height of the fade band just above the composer (part of [bottomInsetPx]). */
    var bottomFadePx: Int = 0
    private val fadePaint = Paint()
    private var fadeShaderKey = 0L
    /**
     * Where messages start below the header (header height plus a gap).
     * Rows scrolled above it fade out over [topFadePx] and are hidden behind
     * a solid band under the header, so text never runs under the title.
     */
    var topInsetPx: Int = 0
        set(value) {
            if (field == value) return
            field = value
            reclamp()
        }
    /** Height of the fade band just below the header (part of [topInsetPx]). */
    var topFadePx: Int = 0
    private val topFadePaint = Paint()
    private var topShaderKey = 0L

    private val scroller = OverScroller(context)
    private val density get() = resources.displayMetrics.density
    private var scroll = 0f
    private var following = true
    private var frame: LayoutFrame? = null
    private val cache = object : LinkedHashMap<ULong, Pair<ULong, RowDisplay>>(32, 0.75f, true) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<ULong, Pair<ULong, RowDisplay>>?) = size > 48
    }
    private val styles = HashMap<UShort, StyleDesc>()
    private val paints = HashMap<UShort, Paint>()
    private val boxPaint = Paint(Paint.ANTI_ALIAS_FLAG)
    private val boxRect = RectF()
    private val barPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply { style = Paint.Style.FILL }
    private val hScroll = HashMap<String, Float>()
    private val clusterBreaks = android.icu.text.BreakIterator.getCharacterInstance()
    private val veilFrom = HashMap<ULong, Int>()
    private val veilAt = HashMap<ULong, Long>()
    private val prevText = HashMap<ULong, String>()
    private var tracking = false
    private var lastY = 0f
    private var lastX = 0f
    private var dragScroller: String? = null
    private val touchSlop = ViewConfiguration.get(context).scaledTouchSlop.toFloat()
    private var downX = 0f
    private var downY = 0f
    private var dragging = false
    private var horizontal = false
    private var downWhileFlinging = false
    private var scrollingNow = false
    private var velocity: VelocityTracker? = null
    private val tap = GestureDetector(context, object : GestureDetector.SimpleOnGestureListener() {
        override fun onDown(e: MotionEvent): Boolean = true
        override fun onSingleTapUp(e: MotionEvent): Boolean {
            onTap()
            hit(e.x, e.y)
            return true
        }
        override fun onLongPress(e: MotionEvent) {
            val row = rowAt(e.y) ?: return
            val display = displayFor(row) ?: return
            if (display.copyText.isNotEmpty()) onCopy(display.copyText)
        }
    })

    init {
        setWillNotDraw(false)
        isFocusable = true
    }

    private val framePending = java.util.concurrent.atomic.AtomicBoolean(false)

    // Diagnostics: when the view was created (open latency breakdown).
    private val createdAt = android.os.SystemClock.uptimeMillis()
    private var firstFrameLogged = false

    /**
     * Called from any thread when the engine has a new frame. A streaming turn
     * can publish many frames between two screen refreshes; only the newest
     * matters, so they collapse into one pass on the next vsync.
     */
    fun requestFrame() {
        if (framePending.compareAndSet(false, true)) {
            postOnAnimation {
                framePending.set(false)
                onFrame()
            }
        }
    }

    fun onFrame() {
        // A frame posted just before the session screen closed its engine
        // lands after the Rust object is gone: nothing to draw any more.
        val next = runCatching { engine?.frame() }.getOrNull() ?: return
        if (next.rowCount() == 0u && next.totalHeight() == 0f && frame == null) {
            frame = next
            invalidate()
            return
        }
        if (!firstFrameLogged && next.rowCount() > 0u) {
            firstFrameLogged = true
            android.util.Log.i(
                "ZeronTranscript",
                "first frame: rows=${next.rowCount()} build=${next.buildMicros() / 1000u}ms at ${android.os.SystemClock.uptimeMillis() - createdAt}ms after view create",
            )
            sh.zeron.android.core.AndroidMeasurer.report()
        }
        syncStyles(next)
        val content = next.totalHeight() * density
        val viewport = (height - bottomInsetPx - topInsetPx).coerceAtLeast(1)
        val maxScroll = max(0f, content - viewport)
        // A finger merely resting on the transcript (tracking but not past
        // the drag slop) is not a scroll: while content is still loading
        // there is nothing to anchor to, so the first real frame must land
        // at the bottom, not freeze the view at the top under the touch.
        // 手指只是按着（未拖过阈值）不算滚动：加载中的帧没有可锚定的行，
        // 否则会卡在顶部第一条消息上。
        if (following && !dragging) {
            scroll = maxScroll
        } else {
            // Older history lands above the visible rows: keep the first
            // visible row put so a prepend doesn't fling the view up.
            // 历史从上方插入时锚住首个可见行，避免视口猛跳。
            val prev = frame
            if (prev != null) {
                val topY = scroll / density
                val bottomY = topY + viewport / density
                for (row in prev.rowsIn(topY, bottomY)) {
                    val index = next.indexOf(row.key) ?: continue
                    val y = next.placement(index)?.y ?: continue
                    val before = scroll
                    scroll = y * density + (scroll - row.y * density)
                    // Content moved under a held touch: the view is not at
                    // the bottom any more, so don't snap back on release.
                    // 内容在按着的手指下挪动了：松手后不要再吸回底部。
                    if (scroll != before) following = false
                    // A fling in flight still aims at old coordinates: restart
                    // it from the corrected spot, keeping its speed.
                    // 惯性滑动还在旧坐标系里跑：从新位置以原速度续上。
                    if (!scroller.isFinished && scroll != before) {
                        val v = scroller.currVelocity
                        scroller.forceFinished(true)
                        scroller.fling(0, scroll.toInt(), 0, v.toInt(), 0, 0, 0, maxScroll.toInt(), 0, height / 4)
                    }
                    break
                }
            }
        }
        scroll = scroll.coerceIn(0f, maxScroll)
        savedScroll = scroll
        frame = next
        refreshUserMarks(next)
        onDistanceFromBottom(maxScroll - scroll)
        onScroll(scroll)
        reportActiveMark()
        invalidate()
    }

    // ── message navigator ───────────────────────────────────────────────

    private var userMarks: List<UserMark> = emptyList()
    private var marksRowCount = -1
    private var marksWidth = -1
    private var marksAt = 0L
    private var activeMark = -1

    /**
     * Re-derive the user-message marks: whenever rows were added or removed
     * or the width changed (reflow moves them), else at most once a second
     * while frames stream in (a streaming reply only changes rows below).
     */
    private fun refreshUserMarks(frame: LayoutFrame) {
        val count = frame.rowCount().toInt()
        val now = android.os.SystemClock.uptimeMillis()
        if (count == marksRowCount && width == marksWidth && now - marksAt < 1_000) return
        marksRowCount = count
        marksWidth = width
        marksAt = now
        val marks = ArrayList<UserMark>()
        for (row in frame.rowsIn(0f, frame.totalHeight() + 1f)) {
            if (row.kind != uniffi.zeron_core.RowKind.USER) continue
            val text = (frame.messageText(row.index) ?: frame.display(row.index)?.copyText).orEmpty()
            marks += UserMark(row.key, row.y, text)
        }
        if (marks != userMarks) {
            userMarks = marks
            activeMark = -1
            onUserMarks(marks)
        }
    }

    /** The message being read (index into the marks), for tests. */
    internal val activeUserMark: Int get() = activeMark
    internal val userMarkCount: Int get() = userMarks.size
    internal fun userMarkKey(i: Int): ULong? = userMarks.getOrNull(i)?.key
    internal fun distanceFromBottomPx(): Float = maxScrollPx() - scroll

    private fun reportActiveMark() {
        if (userMarks.isEmpty()) return
        // The reading line: a little below the header edge, like the desktop
        // rail's own-send inset.
        val top = scroll / density + 24f
        val i = MessageNav.activeIndex(userMarks.map { it.y }, top)
        if (i != activeMark) {
            activeMark = i
            onActiveUserMark(i)
        }
    }

    /** Glide so the row [key] sits just below the header (the navigator's tap). */
    fun scrollToRow(key: ULong) {
        val frame = frame ?: return
        val index = frame.indexOf(key) ?: return
        val placement = frame.placement(index) ?: return
        val target = (placement.y * density - 8f * density).coerceIn(0f, maxScrollPx().toFloat())
        following = false
        scroller.forceFinished(true)
        scroller.startScroll(0, scroll.toInt(), 0, (target - scroll).toInt(), 320)
        savedScroll = target
        postInvalidateOnAnimation()
    }

    private fun reclamp() {
        val content = (frame?.totalHeight() ?: 0f) * density
        val viewport = (height - bottomInsetPx - topInsetPx).coerceAtLeast(1)
        val maxScroll = max(0f, content - viewport)
        if (following && !dragging) scroll = maxScroll
        scroll = scroll.coerceIn(0f, maxScroll)
        onDistanceFromBottom(maxScroll - scroll)
        invalidate()
    }

    fun jumpToBottom() {
        following = true
        val content = (frame?.totalHeight() ?: 0f) * density
        val viewport = (height - bottomInsetPx - topInsetPx).coerceAtLeast(1)
        scroll = max(0f, content - viewport)
        invalidate()
        onDistanceFromBottom(0f)
    }

    override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
        val scale = resources.configuration.fontScale.coerceIn(0.85f, 1.6f)
        if (w > 0) engine?.setViewport(w / density, scale)
        if (savedScroll > 0f && oldw == 0) scroll = savedScroll
    }

    override fun onDraw(canvas: Canvas) {
        val colors = colors ?: return
        val frame = frame ?: return
        val d = density
        val y0 = (scroll - topInsetPx) / d
        val y1 = (scroll - topInsetPx + height) / d + 40f
        val rows = frame.rowsIn(y0 - 80f, y1)
        var animate = false
        canvas.save()
        canvas.translate(0f, topInsetPx - scroll)
        canvas.scale(d, d)
        for (row in rows) {
            val display = displayFor(row) ?: continue
            canvas.save()
            canvas.translate(0f, row.y)
            animate = drawRow(canvas, display, colors) || animate
            canvas.restore()
        }
        canvas.restore()
        drawBottomEdge(canvas, colors)
        drawTopEdge(canvas, colors)
        if (animate) postInvalidateOnAnimation()
    }

    /** The fade edge above the composer, then page color down to the bottom. */
    private fun drawBottomEdge(canvas: Canvas, colors: ZeronColors) {
        if (bottomInsetPx <= 0) return
        val bg = colors.background.toArgb()
        val cut = (height - bottomInsetPx).toFloat()
        val fade = bottomFadePx.coerceIn(0, bottomInsetPx).toFloat()
        val key = (bg.toLong() shl 32) xor (cut.toLong() shl 12) xor fade.toLong()
        if (key != fadeShaderKey) {
            fadeShaderKey = key
            fadePaint.shader = android.graphics.LinearGradient(
                0f, cut, 0f, cut + fade,
                bg and 0x00FFFFFF, bg,
                android.graphics.Shader.TileMode.CLAMP,
            )
        }
        canvas.drawRect(0f, cut, width.toFloat(), height.toFloat(), fadePaint)
    }

    /** Page colour behind the header, then the fade edge down to where rows rest. */
    private fun drawTopEdge(canvas: Canvas, colors: ZeronColors) {
        if (topInsetPx <= 0) return
        val bg = colors.background.toArgb()
        val cut = topInsetPx.toFloat()
        val fade = topFadePx.coerceIn(0, topInsetPx).toFloat()
        val key = (bg.toLong() shl 32) xor (cut.toLong() shl 12) xor fade.toLong()
        if (key != topShaderKey) {
            topShaderKey = key
            topFadePaint.shader = android.graphics.LinearGradient(
                0f, cut - fade, 0f, cut,
                bg, bg and 0x00FFFFFF,
                android.graphics.Shader.TileMode.CLAMP,
            )
        }
        canvas.drawRect(0f, 0f, width.toFloat(), cut, topFadePaint)
    }

    private fun displayFor(row: uniffi.zeron_core.RowPlacement): RowDisplay? {
        val frame = frame ?: return null
        val hit = cache[row.key]
        if (hit != null && hit.first == row.version) return hit.second
        val display = runCatching { frame.display(row.index) }.getOrNull() ?: return null
        val previous = prevText[row.key]
        if (previous != null && display.text.length > previous.length && display.text.startsWith(previous)) {
            veilFrom[row.key] = previous.length
            veilAt[row.key] = SystemClock.uptimeMillis()
        }
        prevText[row.key] = display.text
        cache[row.key] = row.version to display
        return display
    }

    private fun syncStyles(frame: LayoutFrame) {
        for (style in frame.styles()) {
            if (styles[style.id] == null) {
                styles[style.id] = style
                paints[style.id] = Paint(Paint.ANTI_ALIAS_FLAG).apply {
                    typeface = faces[style.face] ?: faces[FaceRole.SANS]
                    textSize = style.size
                    isSubpixelText = true
                    fontFeatureSettings = if (style.ligatures) "\"liga\" 1" else "\"liga\" 0"
                }
            }
        }
    }

    private fun drawRow(canvas: Canvas, display: RowDisplay, colors: ZeronColors): Boolean {
        var animate = false
        val hair = 1f / density.coerceAtLeast(1f)
        drawLayer(canvas, display, colors, scroller = null, hair = hair)
        display.scrollers.forEachIndexed { index, s ->
            val key = "${display.key}:$index"
            val offset = hScroll[key] ?: 0f
            canvas.save()
            canvas.clipRect(s.x, s.y, s.x + s.w, s.y + s.h)
            canvas.translate(s.x - offset, s.y)
            drawLayer(canvas, display, colors, scroller = index.toUInt(), hair = hair)
            canvas.restore()
        }
        val now = SystemClock.uptimeMillis()
        for (w in display.widgets) {
            val host = w.scroller?.toInt()
            canvas.save()
            if (host != null && host < display.scrollers.size) {
                val s = display.scrollers[host]
                val offset = hScroll["${display.key}:$host"] ?: 0f
                canvas.clipRect(s.x, s.y, s.x + s.w, s.y + s.h)
                canvas.translate(s.x - offset, s.y)
            }
            if (drawWidget(canvas, display, w, colors, now)) animate = true
            canvas.restore()
        }
        return animate
    }

    private fun drawLayer(canvas: Canvas, display: RowDisplay, colors: ZeronColors, scroller: UInt?, hair: Float) {
        for (box in display.boxes) {
            if (box.scroller != scroller) continue
            // Reused: a new Paint (native peer) and RectF per box per frame
            // added GC pressure while scrolling.
            val paint = boxPaint
            val color = colors.of(box.color).toArgb()
            val rect = boxRect.apply { set(box.x, box.y, box.x + box.w, box.y + box.h) }
            when (box.style) {
                BoxStyle.FILL -> {
                    paint.color = color
                    paint.style = Paint.Style.FILL
                    canvas.drawRoundRect(rect, box.radius, box.radius, paint)
                }
                BoxStyle.HAIRLINE -> {
                    paint.color = color
                    paint.style = Paint.Style.STROKE
                    paint.strokeWidth = hair
                    val inset = hair / 2f
                    val r = max(0f, box.radius - inset)
                    canvas.drawRoundRect(rect.left + inset, rect.top + inset, rect.right - inset, rect.bottom - inset, r, r, paint)
                }
            }
        }
        val faded = HashSet<Int>()
        display.fades.forEachIndexed { _, fade ->
            if (fade.scroller != scroller) return@forEachIndexed
            val runs = display.runs.mapIndexedNotNull { i, run ->
                if (run.scroller != scroller || faded.contains(i)) return@mapIndexedNotNull null
                if (run.baseline > fade.y && run.baseline <= fade.y + fade.h + 0.5f && (fade.edge == FadeEdge.BOTTOM || run.x < fade.x + fade.w)) i else null
            }
            faded.addAll(runs)
            if (runs.isEmpty()) return@forEachIndexed
            val rect = RectF(fade.x, fade.y, fade.x + fade.w, fade.y + fade.h)
            // Match the iOS painter: the run is drawn in full up to the fade's
            // far edge, and only the fade strip itself is erased. A layer
            // bounded to the strip clips the rest of the line away.
            val layer = if (fade.edge == FadeEdge.TRAILING) {
                canvas.saveLayer(-100_000f, rect.top - 40f, rect.right, rect.bottom + 40f, null)
            } else {
                canvas.saveLayer(rect.left - 40f, rect.top - 40f, rect.right + 40f, rect.bottom + 40f, null)
            }
            for (i in runs) drawRun(canvas, display, i, colors, hair)
            val erase = Paint(Paint.ANTI_ALIAS_FLAG).apply { xfermode = PorterDuffXfermode(PorterDuff.Mode.DST_IN) }
            erase.shader = if (fade.edge == FadeEdge.TRAILING) {
                LinearGradient(rect.left, 0f, rect.right, 0f, 0xFFFFFFFF.toInt(), 0x00FFFFFF, Shader.TileMode.CLAMP)
            } else {
                LinearGradient(0f, rect.top, 0f, rect.bottom, 0xFFFFFFFF.toInt(), 0x00FFFFFF, Shader.TileMode.CLAMP)
            }
            canvas.drawRect(rect, erase)
            canvas.restoreToCount(layer)
        }
        display.runs.forEachIndexed { i, run ->
            if (run.scroller != scroller || faded.contains(i)) return@forEachIndexed
            drawRun(canvas, display, i, colors, hair)
        }
    }

    private fun drawRun(canvas: Canvas, display: RowDisplay, index: Int, colors: ZeronColors, hair: Float) {
        val run = display.runs[index]
        val start = run.start.toInt()
        val end = start + run.len.toInt()
        if (start < 0 || end > display.text.length || start > end) return
        val paint = (paints[run.style] ?: paints.values.firstOrNull() ?: Paint(Paint.ANTI_ALIAS_FLAG).apply { textSize = 16.5f }).also {
            it.color = colors.of(run.color).toArgb()
        }
        val veil = veilFrom[display.key]
        val born = veilAt[display.key]
        if (veil != null && born != null && start >= veil) {
            val t = ((SystemClock.uptimeMillis() - born) / 220f).coerceIn(0f, 1f)
            paint.alpha = (t * 255).toInt()
            if (t < 1f) postInvalidateOnAnimation()
        } else {
            paint.alpha = 255
        }
        val runStyle = styles[run.style]
        if (runStyle != null && FontChain.isMono(runStyle.face)) {
            val slice = display.text.substring(start, end)
            if (FontChain.hasWide(slice)) {
                drawMonoWide(canvas, slice, run.x, run.baseline, paint)
            } else {
                canvas.drawText(slice, run.x, run.baseline, paint)
            }
        } else {
            // Draw straight from the row text: no substring per run per frame.
            canvas.drawText(display.text, start, end, run.x, run.baseline, paint)
        }
        if (run.decoration != Decoration.NONE) {
            val y = if (run.decoration == Decoration.UNDERLINE) run.baseline + 2f else run.baseline - 5f
            val bar = barPaint.apply { color = paint.color }
            canvas.drawRect(run.x, y, run.x + run.width, y + max(hair, 1f / density), bar)
        }
    }

    /**
     * Mono text with wide (CJK) characters: each wide cluster sits centered in
     * exactly two Geist Mono cells — the width [sh.zeron.android.core.AndroidMeasurer]
     * reported to the Rust layout — so columns stay aligned.
     */
    private fun drawMonoWide(canvas: Canvas, text: String, x0: Float, baseline: Float, paint: Paint) {
        val cell = FontChain.cellWidth(paint)
        val it = clusterBreaks
        it.setText(text)
        var x = x0
        var runStart = 0
        var start = it.first()
        var end = it.next()
        fun flush(upTo: Int) {
            if (upTo > runStart) {
                val seg = text.substring(runStart, upTo)
                canvas.drawText(seg, x, baseline, paint)
                x += paint.measureText(seg)
            }
        }
        while (end != android.icu.text.BreakIterator.DONE) {
            if (FontChain.isWide(text.codePointAt(start))) {
                flush(start)
                val cluster = text.substring(start, end)
                val adv = paint.measureText(cluster)
                canvas.drawText(cluster, x + (cell * 2f - adv) / 2f, baseline, paint)
                x += cell * 2f
                runStart = end
            }
            start = end
            end = it.next()
        }
        flush(text.length)
    }

    private fun drawWidget(canvas: Canvas, display: RowDisplay, w: Widget, colors: ZeronColors, now: Long): Boolean {
        val kind = w.kind
        var animate = false
        when (kind) {
            WidgetKind.CopyCode -> drawCopyButton(canvas, w, colors, copied = copiedCode == copyKey(display, w))
            is WidgetKind.Disclosure -> Unit
            is WidgetKind.Chevron -> {
                val px = max(w.w, w.h) * density
                val bmp = sh.zeron.android.design.IconAssets.bitmap(context, "tool-alt-arrow-down", px.toInt().coerceAtLeast(1), colors.secondary.toArgb())
                if (bmp != null) {
                    canvas.save()
                    canvas.translate(w.x + w.w / 2f, w.y + w.h / 2f)
                    if (!kind.expanded) canvas.rotate(-90f)
                    val dst = RectF(-w.w / 2f, -w.h / 2f, w.w / 2f, w.h / 2f)
                    canvas.drawBitmap(bmp, null, dst, Paint(Paint.ANTI_ALIAS_FLAG))
                    canvas.restore()
                }
            }
            is WidgetKind.ToolStatus -> {
                if (kind.running) {
                    drawSpinner(canvas, w.x, w.y, w.w, w.h, colors, trailer = false, now = now)
                    animate = true
                } else {
                    val paint = stroke(if (kind.failed) colors.danger.toArgb() else colors.tertiary.toArgb(), 1.7f)
                    val path = Path()
                    val cx = w.x + w.w / 2f
                    val cy = w.y + w.h / 2f
                    if (kind.failed) {
                        path.moveTo(cx - 3.5f, cy - 3.5f); path.lineTo(cx + 3.5f, cy + 3.5f)
                        path.moveTo(cx + 3.5f, cy - 3.5f); path.lineTo(cx - 3.5f, cy + 3.5f)
                    } else {
                        path.moveTo(cx - 4f, cy); path.lineTo(cx - 1f, cy + 3f); path.lineTo(cx + 4.5f, cy - 3.5f)
                    }
                    canvas.drawPath(path, paint)
                }
            }
            is WidgetKind.Image -> {
                val bmp = imageFor(kind.reference)
                val rect = RectF(w.x, w.y, w.x + w.w, w.y + w.h)
                if (bmp == null) {
                    requestImage(kind.reference)
                    canvas.drawRoundRect(rect, 12f, 12f, Paint(Paint.ANTI_ALIAS_FLAG).apply { color = colors.chip.toArgb() })
                } else {
                    val shader = android.graphics.BitmapShader(bmp, Shader.TileMode.CLAMP, Shader.TileMode.CLAMP)
                    val sx = w.w / bmp.width
                    val sy = w.h / bmp.height
                    val m = android.graphics.Matrix()
                    val scale = max(sx, sy)
                    m.setScale(scale, scale)
                    m.postTranslate(w.x - (bmp.width * scale - w.w) / 2f, w.y - (bmp.height * scale - w.h) / 2f)
                    shader.setLocalMatrix(m)
                    canvas.drawRoundRect(rect, if (min(w.w, w.h) > 120f) 14f else 12f, if (min(w.w, w.h) > 120f) 14f else 12f, Paint(Paint.ANTI_ALIAS_FLAG).apply { this.shader = shader })
                }
            }
            WidgetKind.Spinner -> {
                drawSpinner(canvas, w.x, w.y, w.w, w.h, colors, trailer = false, now = now)
                animate = true
            }
            is WidgetKind.Working -> {
                drawSpinner(canvas, w.x, w.y + (w.h - 14f) / 2f, 14f, 14f, colors, trailer = true, now = now)
                val word = context.getString(if (kind.streaming) sh.zeron.android.R.string.status_writing else sh.zeron.android.R.string.status_working)
                val secs = kind.sinceMs?.let { max(0L, (System.currentTimeMillis() - it) / 1000) } ?: 0L
                val paint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
                    typeface = faces[FaceRole.SANS_MEDIUM] ?: faces[FaceRole.SANS]
                    textSize = 13.5f
                    color = colors.secondary.toArgb()
                }
                val label = "$word…"
                val x = w.x + 22f
                val baseline = w.y + w.h / 2f - (paint.ascent() + paint.descent()) / 2f
                canvas.drawText(label, x, baseline, paint)
                // iOS WorkingIndicatorView: the elapsed time follows, fainter.
                if (secs > 0) {
                    val tx = x + paint.measureText(label)
                    paint.color = colors.tertiary.toArgb()
                    canvas.drawText("  ${ElapsedFormat.format(secs, resources)}", tx, baseline, paint)
                }
                animate = true
            }
            is WidgetKind.TurnEnd -> {
                // How the last turn ended: the home rows' glyph (StatusMark's
                // check / dot, same geometry and colours), its word, then when
                // it ended in the rows' relative-time format, fainter — the
                // working row's layout, so one reads as the other's successor.
                val box = 14f
                drawStatusMark(canvas, w.x, w.y + (w.h - box) / 2f, box, kind.failed, colors)
                val paint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
                    typeface = faces[FaceRole.SANS_MEDIUM] ?: faces[FaceRole.SANS]
                    textSize = 13.5f
                    color = colors.secondary.toArgb()
                }
                val word = context.getString(if (kind.failed) sh.zeron.android.R.string.status_failed else sh.zeron.android.R.string.status_done)
                val x = w.x + 22f
                val baseline = w.y + w.h / 2f - (paint.ascent() + paint.descent()) / 2f
                canvas.drawText(word, x, baseline, paint)
                paint.color = colors.tertiary.toArgb()
                val time = RelativeTime.label(kind.atMs, System.currentTimeMillis(), resources)
                canvas.drawText("  $time", x + paint.measureText(word), baseline, paint)
                // The label ages by the minute: repaint now and then (no relayout).
                postInvalidateDelayed(30_000)
            }
            is WidgetKind.HistoryPending -> {
                // Only the newest rows are here; the rest of the transcript
                // is still on its way (a Direct link sends it whole, which
                // over a slow relay takes a while). Centred, faint: spinner,
                // "Loading earlier messages…", and how much has come in.
                val paint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
                    typeface = faces[FaceRole.SANS] ?: faces[FaceRole.SANS_MEDIUM]
                    textSize = 12.5f
                    color = colors.tertiary.toArgb()
                }
                val word = context.getString(sh.zeron.android.R.string.history_loading)
                val got = kind.receivedBytes.toLong()
                val size = if (got > 0) "  " + context.getString(
                    sh.zeron.android.R.string.history_received,
                    android.text.format.Formatter.formatShortFileSize(context, got),
                ) else ""
                val spin = 14f
                val textW = paint.measureText(word) + paint.measureText(size)
                val total = spin + 8f + textW
                val x0 = w.x + max(0f, (w.w - total) / 2f)
                drawSpinner(canvas, x0, w.y + (w.h - spin) / 2f, spin, spin, colors, trailer = false, now = now)
                val baseline = w.y + w.h / 2f - (paint.ascent() + paint.descent()) / 2f
                val tx = x0 + spin + 8f
                canvas.drawText(word, tx, baseline, paint)
                if (size.isNotEmpty()) canvas.drawText(size, tx + paint.measureText(word), baseline, paint)
                animate = true
            }
            is WidgetKind.Detail -> Unit
            is WidgetKind.Icon -> {
                val px = max(w.w, w.h) * density
                val tint = colors.of(kind.color).toArgb()
                val bmp = sh.zeron.android.design.IconAssets.bitmap(context, kind.name, px.toInt().coerceAtLeast(1), tint)
                if (bmp != null) {
                    val dst = RectF(w.x, w.y, w.x + w.w, w.y + w.h)
                    canvas.drawBitmap(bmp, null, dst, Paint(Paint.ANTI_ALIAS_FLAG))
                }
            }
            is WidgetKind.ToolRail -> {
                val paint = stroke(colors.of(ColorRole.TOOL_RAIL).toArgb(), 1f)
                for (i in kind.tops.indices) {
                    val top = w.y + kind.tops[i]
                    val mid = top + kind.rowMid
                    val trunk = w.x + kind.trunkX
                    val path = Path()
                    path.moveTo(trunk, top)
                    path.lineTo(trunk, mid - kind.bend)
                    path.quadTo(trunk, mid, trunk + kind.bend, mid)
                    path.lineTo(w.x + kind.branchEnd, mid)
                    if (i + 1 < kind.tops.size) {
                        path.moveTo(trunk, mid - kind.bend)
                        path.lineTo(trunk, top + kind.heights[i])
                    }
                    canvas.drawPath(path, paint)
                }
            }
            is WidgetKind.ToolToggle -> Unit
            WidgetKind.Shimmer -> {
                val phase = ((now % 3400L) / 3400f)
                val band = w.w * 0.35f
                val x = w.x - band + (w.w + band * 2) * phase
                val paint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
                    shader = LinearGradient(x, w.y, x + band, w.y, intArrayOf(0x00FFFFFF, 0x66FFFFFF, 0x00FFFFFF), floatArrayOf(0f, 0.5f, 1f), Shader.TileMode.CLAMP)
                }
                canvas.save()
                canvas.clipRect(w.x, w.y, w.x + w.w, w.y + w.h)
                canvas.drawRect(w.x, w.y, w.x + w.w, w.y + w.h, paint)
                canvas.restore()
                animate = true
            }
        }
        return animate
    }

    private fun drawSpinner(canvas: Canvas, x: Float, y: Float, w: Float, h: Float, colors: ZeronColors, trailer: Boolean, now: Long) {
        val t = (now % 750L) / 750f
        val cols = if (trailer) 3 else 2
        val rows = if (trailer) 3 else 3
        val d = if (trailer) 3.5f else 3.5f
        val gap = if (trailer) 1.75f else 1.5f
        val gridW = d * cols + gap * (cols - 1)
        val gridH = d * rows + gap * (rows - 1)
        val ox = x + w / 2f - gridW / 2f
        val oy = y + h / 2f - gridH / 2f
        val tints = if (!trailer) {
            colors.glyph.map { it.toArgb() }.toIntArray()
        } else intArrayOf(0xFFB6D3EF.toInt(), 0xFFEDB185.toInt(), 0xFFF888A0.toInt())
        val ring = arrayOf(intArrayOf(0, 1), intArrayOf(5, 2), intArrayOf(4, 3))
        val paint = Paint(Paint.ANTI_ALIAS_FLAG)
        for (i in 0 until cols * rows) {
            val row = i / cols
            val col = i % cols
            val phase = if (!trailer) ring[row][col] / 6f else (2 - row + abs(col - 1)) / 4f
            val alpha = (glyphOpacity(t + phase) * 255).toInt().coerceIn(0, 255)
            paint.color = (tints[row] and 0x00FFFFFF) or (alpha shl 24)
            val cx = ox + col * (d + gap) + d / 2f
            val cy = oy + row * (d + gap) + d / 2f
            canvas.drawCircle(cx, cy, d / 2f, paint)
        }
    }

    /**
     * [sh.zeron.android.design.StatusMark]'s done check / failed dot on a
     * canvas, in a `size`-square box at (x, y): the same geometry, and the
     * home rows' `done` / `failed` colours.
     */
    private fun drawStatusMark(canvas: Canvas, x: Float, y: Float, size: Float, failed: Boolean, colors: ZeronColors) {
        val cx = x + size / 2f
        val cy = y + size / 2f
        if (failed) {
            canvas.drawCircle(cx, cy, size * 0.29f, Paint(Paint.ANTI_ALIAS_FLAG).apply { color = colors.failed.toArgb() })
            return
        }
        val s = size / 16f * 1.05f
        val ox = cx - 8f * s
        val oy = cy - 8f * s
        val path = Path().apply {
            moveTo(ox + 3.5f * s, oy + 8.5f * s)
            lineTo(ox + 6.5f * s, oy + 11.5f * s)
            lineTo(ox + 12.5f * s, oy + 4.5f * s)
        }
        canvas.drawPath(path, stroke(colors.done.toArgb(), 1.6f * (size / 12f)))
    }

    private fun stroke(color: Int, width: Float = 1.4f) = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        this.color = color
        style = Paint.Style.STROKE
        strokeWidth = width
        strokeCap = Paint.Cap.ROUND
        strokeJoin = Paint.Join.ROUND
    }

    /**
     * The code block's copy control: a square icon button (the layout hands a
     * 44 x 32 header cell, which the old inset-based glyph stretched into a
     * flat, wide pair of boxes). Idle it is a two-sheet copy glyph on a faint
     * rounded tile; for [COPIED_MS] after a tap it turns into a check on a
     * success-tinted tile, then back (desktop `code_copy_button`).
     */
    private fun drawCopyButton(canvas: Canvas, w: Widget, colors: ZeronColors, copied: Boolean) {
        val r = copyButtonRect(w)
        val tone = if (copied) colors.success else colors.tertiary
        boxPaint.style = Paint.Style.FILL
        boxPaint.color = (if (copied) colors.success.copy(alpha = 0.14f) else colors.text.copy(alpha = 0.06f)).toArgb()
        canvas.drawRoundRect(r, 7f, 7f, boxPaint)
        val cx = r.centerX()
        val cy = r.centerY()
        if (copied) {
            val path = Path()
            path.moveTo(cx - 4.5f, cy + 0.2f); path.lineTo(cx - 1.3f, cy + 3.4f); path.lineTo(cx + 4.8f, cy - 3.6f)
            canvas.drawPath(path, stroke(tone.toArgb(), 1.7f))
        } else {
            // Two 8.5 x 8.5 sheets offset by 3: the back one peeks top-right.
            val g = 8.5f
            val o = 3f
            val left = cx - (g + o) / 2f
            val top = cy - (g + o) / 2f
            val paint = stroke(tone.toArgb(), 1.4f)
            canvas.save()
            canvas.clipOutRect(left - 1.4f, top + o - 1.4f, left + g + 1.4f, top + o + g + 1.4f)
            canvas.drawRoundRect(left + o, top, left + o + g, top + g, 2f, 2f, paint)
            canvas.restore()
            canvas.drawRoundRect(left, top + o, left + g, top + o + g, 2f, 2f, paint)
        }
    }

    /** The visible button inside the (larger, tappable) header cell: 26 x 26, right-aligned. */
    private fun copyButtonRect(w: Widget): RectF {
        val side = min(26f, w.h - 4f).coerceAtLeast(12f)
        val right = w.x + w.w - 5f
        val top = w.y + (w.h - side) / 2f
        return RectF(right - side, top, right, top + side)
    }

    private fun copyKey(display: RowDisplay, w: Widget) = "${display.key}:${w.x.toInt()}:${w.y.toInt()}"

    /** Which code block's button shows the check right now (`row key:x:y`), if any. */
    internal var copiedCode: String? = null
        private set
    private val clearCopied = Runnable { copiedCode = null; invalidate() }

    private fun markCopied(key: String) {
        copiedCode = key
        removeCallbacks(clearCopied)
        postDelayed(clearCopied, COPIED_MS)
        invalidate()
    }

    /** Keys of rows holding a code block's copy button, top first (tests, screenshots). */
    internal fun codeRowKeys(): List<ULong> {
        val frame = frame ?: return emptyList()
        return frame.rowsIn(0f, frame.totalHeight() + 1f).filter { row ->
            frame.display(row.index)?.widgets?.any { it.kind == WidgetKind.CopyCode } == true
        }.map { it.key }
    }

    /** Centres (view px) of the code-copy buttons on screen, top first (tests, screenshots). */
    internal fun copyButtonCenters(): List<android.graphics.PointF> {
        val frame = frame ?: return emptyList()
        val d = density
        val y0 = (scroll - topInsetPx) / d
        val out = ArrayList<android.graphics.PointF>()
        for (row in frame.rowsIn(y0, y0 + height / d)) {
            val display = displayFor(row) ?: continue
            for (w in display.widgets) {
                if (w.kind != WidgetKind.CopyCode) continue
                val r = copyButtonRect(w)
                val y = (row.y + r.centerY()) * d - scroll + topInsetPx
                if (y > topInsetPx && y < height - bottomInsetPx) out += android.graphics.PointF(r.centerX() * d, y)
            }
        }
        return out
    }

    private fun drawMiniIcon(canvas: Canvas, w: Widget, color: Int, block: (Canvas, RectF) -> Unit) {
        val inset = min(w.w, w.h) * 0.22f
        block(canvas, RectF(w.x + inset, w.y + inset, w.x + w.w - inset, w.y + w.h - inset))
    }

    /**
     * Touch handling, iOS-style: nothing moves until the finger passes the
     * system touch slop, then the gesture locks to one axis (a code block's
     * horizontal scroller or the transcript) for its whole length. Before this
     * the list followed every pixel from the first touch, so a tap with a tiny
     * wobble both nudged the list and fired the tap, and a slightly diagonal
     * drag over a code block flipped between scrolling it and the transcript.
     * A touch that lands while the list is still flinging only stops it.
     */
    override fun onTouchEvent(event: MotionEvent): Boolean {
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                tracking = true
                dragging = false
                downWhileFlinging = !scroller.isFinished
                downX = event.x
                downY = event.y
                lastX = event.x
                lastY = event.y
                dragScroller = scrollerAt(event.x, event.y)
                scroller.forceFinished(true)
                velocity?.recycle()
                velocity = VelocityTracker.obtain()
                velocity?.addMovement(event)
                parent.requestDisallowInterceptTouchEvent(true)
                if (!downWhileFlinging) tap.onTouchEvent(event)
            }
            MotionEvent.ACTION_MOVE -> {
                velocity?.addMovement(event)
                if (!dragging) {
                    val tdx = event.x - downX
                    val tdy = event.y - downY
                    if (hypot(tdx, tdy) <= touchSlop) {
                        if (!downWhileFlinging) tap.onTouchEvent(event)
                        return true
                    }
                    dragging = true
                    horizontal = dragScroller != null && abs(tdx) > abs(tdy)
                    // Start from the slop edge so the content does not jump.
                    lastX = if (horizontal) downX + sign(tdx) * touchSlop else event.x
                    lastY = if (horizontal) event.y else downY + sign(tdy) * touchSlop
                    cancelTap(event)
                    setScrolling(true)
                }
                val dx = event.x - lastX
                val dy = event.y - lastY
                val sc = dragScroller
                if (horizontal && sc != null) {
                    val max = maxScroll(sc)
                    hScroll[sc] = ((hScroll[sc] ?: 0f) - dx / density).coerceIn(0f, max)
                    invalidate()
                } else {
                    following = false
                    scrollBy(-dy)
                }
                lastX = event.x
                lastY = event.y
            }
            MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> {
                velocity?.addMovement(event)
                velocity?.computeCurrentVelocity(1000)
                val vy = velocity?.yVelocity ?: 0f
                var flinging = false
                if (dragging && !horizontal && event.actionMasked == MotionEvent.ACTION_UP && abs(vy) > 80f) {
                    scroller.fling(0, scroll.toInt(), 0, (-vy).toInt(), 0, 0, 0, maxScrollPx(), 0, height / 4)
                    postInvalidateOnAnimation()
                    flinging = true
                }
                // A touch that ends away from where it began is never a tap,
                // even if its move events were merged away.
                val stayed = hypot(event.x - downX, event.y - downY) <= touchSlop
                if (!dragging && !downWhileFlinging && stayed) tap.onTouchEvent(event) else cancelTap(event)
                if (!flinging) setScrolling(false)
                tracking = false
                dragging = false
                // Repairs any state where the view still follows but drifted
                // off the bottom while the touch held (e.g. a frame landed
                // between DOWN and its anchor rows existing).
                // 修复按住期间跟随中却偏离底部的残留状态。
                if (!flinging && following) {
                    scroll = maxScrollPx().toFloat()
                    onDistanceFromBottom(0f)
                    onScroll(scroll)
                    invalidate()
                }
                dragScroller = null
                velocity?.recycle()
                velocity = null
            }
            else -> if (!dragging && !downWhileFlinging) tap.onTouchEvent(event)
        }
        return true
    }

    private fun cancelTap(event: MotionEvent) {
        val cancel = MotionEvent.obtain(event)
        cancel.action = MotionEvent.ACTION_CANCEL
        tap.onTouchEvent(cancel)
        cancel.recycle()
    }

    private fun setScrolling(active: Boolean) {
        if (active == scrollingNow) return
        scrollingNow = active
        onScrollActive(active)
    }

    override fun computeScroll() {
        if (scroller.computeScrollOffset()) {
            scroll = scroller.currY.toFloat().coerceIn(0f, maxScrollPx().toFloat())
            val maxScroll = maxScrollPx().toFloat()
            following = maxScroll - scroll < 24f * density
            onDistanceFromBottom(maxScroll - scroll)
            onScroll(scroll)
            reportActiveMark()
            postInvalidateOnAnimation()
        } else if (scrollingNow && !tracking) {
            setScrolling(false)
        }
    }

    private fun scrollBy(dy: Float) {
        val maxScroll = maxScrollPx().toFloat()
        scroll = (scroll + dy).coerceIn(0f, maxScroll)
        // Re-follow only when the drag lands right at the bottom edge: a
        // generous threshold made small scrolls-up snap back on the next frame.
        // 只有拖到边才恢复跟随：阈值太宽，小幅上滑会在下一帧被吸回去。
        following = maxScroll - scroll < 24f * density
        onDistanceFromBottom(maxScroll - scroll)
        onScroll(scroll)
        reportActiveMark()
        savedScroll = scroll
        invalidate()
    }

    private fun maxScrollPx(): Int {
        val content = (frame?.totalHeight() ?: 0f) * density
        val viewport = (height - bottomInsetPx - topInsetPx).coerceAtLeast(1)
        return max(0f, content - viewport).toInt()
    }

    private fun rowAt(yPx: Float): uniffi.zeron_core.RowPlacement? {
        val frame = frame ?: return null
        // Rows faded out above the composer aren't tappable.
        if (bottomInsetPx > 0 && yPx > height - bottomInsetPx) return null
        // Nor rows hidden behind the header.
        if (topInsetPx > 0 && yPx < topInsetPx - topFadePx) return null
        val y = (scroll + yPx - topInsetPx) / density
        return frame.rowsIn(y, y + 1f).firstOrNull()
    }

    private fun hit(xPx: Float, yPx: Float) {
        val row = rowAt(yPx) ?: return
        val display = displayFor(row) ?: return
        val x = xPx / density
        val y = (scroll + yPx - topInsetPx) / density - row.y
        for (w in display.widgets.asReversed()) {
            val rect = widgetRect(display, w)
            if (x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom) {
                when (val kind = w.kind) {
                    is WidgetKind.Disclosure, is WidgetKind.Chevron -> {
                        onToggle(display.key)
                        return
                    }
                    is WidgetKind.ToolToggle -> {
                        onToggleDetail(display.key, kind.detail, kind.open)
                        return
                    }
                    WidgetKind.CopyCode -> {
                        onCopy(w.payload ?: "")
                        markCopied(copyKey(display, w))
                        return
                    }
                    is WidgetKind.Detail -> {
                        onDetail(kind.title, w.payload ?: "")
                        return
                    }
                    is WidgetKind.Image -> {
                        imageFor(kind.reference)?.let(onImage) ?: onImageMiss()
                        return
                    }
                    else -> Unit
                }
            }
        }
        for (link in display.links) {
            var lx = x
            var ly = y
            val s = link.scroller?.toInt()
            if (s != null && s < display.scrollers.size) {
                val sc = display.scrollers[s]
                val offset = hScroll["${display.key}:$s"] ?: 0f
                lx = x - sc.x + offset
                ly = y - sc.y
            }
            if (lx >= link.x - 4 && lx <= link.x + link.w + 4 && ly >= link.y - 2 && ly <= link.y + link.h + 2) {
                onLink(link.url)
                return
            }
        }
    }

    private fun widgetRect(display: RowDisplay, w: Widget): RectF {
        val s = w.scroller?.toInt()
        if (s != null && s < display.scrollers.size) {
            val sc = display.scrollers[s]
            val offset = hScroll["${display.key}:$s"] ?: 0f
            return RectF(sc.x + w.x - offset, sc.y + w.y, sc.x + w.x - offset + w.w, sc.y + w.y + w.h)
        }
        return RectF(w.x, w.y, w.x + w.w, w.y + w.h)
    }

    private fun scrollerAt(xPx: Float, yPx: Float): String? {
        val row = rowAt(yPx) ?: return null
        val display = displayFor(row) ?: return null
        val x = xPx / density
        val y = (scroll + yPx - topInsetPx) / density - row.y
        display.scrollers.forEachIndexed { i, s ->
            if (s.contentWidth > s.w + 2 && x >= s.x && x <= s.x + s.w && y >= s.y && y <= s.y + s.h) {
                return "${display.key}:$i"
            }
        }
        return null
    }

    private fun maxScroll(key: String): Float {
        val (rowKey, index) = key.split(":").let { it[0].toULong() to it[1].toInt() }
        val display = cache[rowKey]?.second ?: return 0f
        val s = display.scrollers.getOrNull(index) ?: return 0f
        return max(0f, s.contentWidth - s.w)
    }

    private fun androidx.compose.ui.graphics.Color.toArgb(): Int {
        return android.graphics.Color.argb(
            (alpha * 255).toInt().coerceIn(0, 255),
            (red * 255).toInt().coerceIn(0, 255),
            (green * 255).toInt().coerceIn(0, 255),
            (blue * 255).toInt().coerceIn(0, 255),
        )
    }
}

