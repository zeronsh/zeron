package sh.zeron.android.tools

import android.annotation.SuppressLint
import android.content.Context
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.Rect
import android.text.InputType
import android.util.TypedValue
import android.view.GestureDetector
import android.view.HapticFeedbackConstants
import android.view.KeyCharacterMap
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.View
import android.view.inputmethod.BaseInputConnection
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection
import android.view.inputmethod.InputMethodManager
import android.widget.OverScroller
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import sh.zeron.android.core.Fonts
import uniffi.zeron_core.FaceRole
import uniffi.zeron_core.TerminalFrame
import uniffi.zeron_core.TerminalKey
import uniffi.zeron_core.TerminalPalette
import uniffi.zeron_core.TerminalRun
import uniffi.zeron_core.TerminalSelection
import kotlin.math.ceil
import kotlin.math.floor
import kotlin.math.max

/** The extra-keys row's sticky Ctrl/Alt: armed until the next key or character. */
class StickyKeys {
    var ctrl by mutableStateOf(false)
    var alt by mutableStateOf(false)

    /** The armed modifiers, disarming them. */
    fun take(): Pair<Boolean, Boolean> = (ctrl to alt).also {
        ctrl = false
        alt = false
    }
}

/** Typed text: newlines become Enter; sticky modifiers apply to the first character only. */
fun TerminalSession.typeText(text: String, sticky: StickyKeys, ctrl: Boolean = false, alt: Boolean = false) {
    if (text.isEmpty() || !isAttached) return
    var (sc, sa) = sticky.take()
    text.replace("\r\n", "\n").replace('\r', '\n').split('\n').forEachIndexed { i, seg ->
        if (i > 0) {
            screen.writeKey(TerminalKey.ENTER, ctrl || sc, alt || sa, false)
            sc = false
            sa = false
        }
        if (seg.isEmpty()) return@forEachIndexed
        if (sc || sa) {
            val n = Character.charCount(seg.codePointAt(0))
            screen.writeText(seg.substring(0, n), ctrl || sc, alt || sa)
            sc = false
            sa = false
            if (seg.length > n) screen.writeText(seg.substring(n), ctrl, alt)
        } else {
            screen.writeText(seg, ctrl, alt)
        }
    }
}

fun TerminalSession.pressKey(key: TerminalKey, sticky: StickyKeys, ctrl: Boolean = false, alt: Boolean = false, shift: Boolean = false) {
    if (!isAttached) return
    val (sc, sa) = sticky.take()
    screen.writeKey(key, ctrl || sc, alt || sa, shift)
}

/** Hardware key → terminal key, for keys that aren't text. */
fun terminalKey(keyCode: Int): TerminalKey? = when (keyCode) {
    KeyEvent.KEYCODE_ENTER, KeyEvent.KEYCODE_NUMPAD_ENTER -> TerminalKey.ENTER
    KeyEvent.KEYCODE_DEL -> TerminalKey.BACKSPACE
    KeyEvent.KEYCODE_FORWARD_DEL -> TerminalKey.DELETE
    KeyEvent.KEYCODE_TAB -> TerminalKey.TAB
    KeyEvent.KEYCODE_ESCAPE -> TerminalKey.ESCAPE
    KeyEvent.KEYCODE_DPAD_UP -> TerminalKey.UP
    KeyEvent.KEYCODE_DPAD_DOWN -> TerminalKey.DOWN
    KeyEvent.KEYCODE_DPAD_LEFT -> TerminalKey.LEFT
    KeyEvent.KEYCODE_DPAD_RIGHT -> TerminalKey.RIGHT
    KeyEvent.KEYCODE_MOVE_HOME -> TerminalKey.HOME
    KeyEvent.KEYCODE_MOVE_END -> TerminalKey.END
    KeyEvent.KEYCODE_INSERT -> TerminalKey.INSERT
    KeyEvent.KEYCODE_PAGE_UP -> TerminalKey.PAGE_UP
    KeyEvent.KEYCODE_PAGE_DOWN -> TerminalKey.PAGE_DOWN
    in KeyEvent.KEYCODE_F1..KeyEvent.KEYCODE_F12 -> TerminalKey.entries[TerminalKey.F1.ordinal + keyCode - KeyEvent.KEYCODE_F1]
    else -> null
}

/** Whole cells that fit in a [width]×[height] px view with [pad] on every side. */
fun terminalGrid(width: Int, height: Int, pad: Float, cellWidth: Float, lineHeight: Float): Pair<Int, Int> {
    val cols = floor((width - 2 * pad) / cellWidth).toInt().coerceIn(2, 1000)
    val rows = floor((height - 2 * pad) / lineHeight).toInt().coerceIn(1, 1000)
    return cols to rows
}

private val AnsiDark = longArrayOf(
    0xFF242424, 0xFFF87171, 0xFF4ADE80, 0xFFFACC15, 0xFF60A5FA, 0xFFC084FC, 0xFF22D3EE, 0xFFD4D4D8,
    0xFF52525B, 0xFFFCA5A5, 0xFF86EFAC, 0xFFFDE047, 0xFF93C5FD, 0xFFD8B4FE, 0xFF67E8F9, 0xFFFAFAFA,
)

private val AnsiLight = longArrayOf(
    0xFF1F1F1F, 0xFFDC2626, 0xFF16A34A, 0xFFB45309, 0xFF2563EB, 0xFF9333EA, 0xFF0E7490, 0xFF3F3F46,
    0xFF71717A, 0xFFB91C1C, 0xFF15803D, 0xFF92400E, 0xFF1D4ED8, 0xFF7E22CE, 0xFF155E75, 0xFF18181B,
)

/**
 * Zeron Dark / Light's terminal colors (crates/theme builtins) over the
 * app's own [background], so the terminal reads as part of the page.
 */
fun terminalPalette(dark: Boolean, background: Int, cursor: Int): TerminalPalette = TerminalPalette(
    foreground = if (dark) 0xFFE8E8EAu else 0xFF303035u,
    background = background.toUInt(),
    ansi = (if (dark) AnsiDark else AnsiLight).map { it.toUInt() },
    cursor = cursor.toUInt(),
    selection = if (dark) 0x38FFFFFFu else 0x29000000u,
    light = !dark,
)

/**
 * Paints a [TerminalSession]'s grid in Geist Mono and turns touch, the soft
 * keyboard and hardware keys into terminal input. Vertical drags scroll the
 * scrollback, long-press selects a word (drag to extend), a tap shows the
 * keyboard or clears the selection.
 */
@SuppressLint("ViewConstructor")
class TerminalView(context: Context, private val sticky: StickyKeys) : View(context) {
    /** Called when the grid that fits changes (cols, rows). */
    var onGrid: ((Int, Int) -> Unit)? = null

    var session: TerminalSession? = null
        set(value) {
            if (field === value) return
            field?.onFrame = null
            field = value
            value?.onFrame = { invalidate() }
            if (value != null && cols > 0) value.resize(cols, rows)
            invalidate()
        }

    private val metrics = resources.displayMetrics
    private val density = metrics.density
    private val pad = 8 * density
    private val textSize = TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_SP, 13f, metrics)
    private val regular = Fonts.paint(FaceRole.MONO, textSize, false)
    private val bold = Fonts.paint(FaceRole.MONO_SEMIBOLD, textSize, false)
    private val italic = Fonts.paint(FaceRole.MONO_ITALIC, textSize, false)
    private val boldItalic = Fonts.paint(FaceRole.MONO_ITALIC, textSize, false).apply { isFakeBoldText = true }
    private val fill = Paint()
    private val outline = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        style = Paint.Style.STROKE
        strokeWidth = max(1f, density)
    }

    val cellWidth: Float = regular.measureText("M")
    val lineHeight: Float
    private val baseline: Float

    init {
        val fm = regular.fontMetrics
        val glyph = fm.descent - fm.ascent
        lineHeight = ceil(max(textSize * 1.3f, glyph))
        baseline = (lineHeight - glyph) / 2 - fm.ascent
        isFocusable = true
        isFocusableInTouchMode = true
    }

    var cols = 0
        private set
    var rows = 0
        private set

    private var background = 0xFF000000.toInt()
    private var cursorColor = 0xFFFFFFFF.toInt()

    fun setColors(background: Int, cursor: Int) {
        if (background == this.background && cursor == cursorColor) return
        this.background = background
        cursorColor = cursor
        invalidate()
    }

    // ── layout & paint ─────────────────────────────────────────────────────

    override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
        super.onSizeChanged(w, h, oldw, oldh)
        if (w <= 0 || h <= 0) return
        val (c, r) = terminalGrid(w, h, pad, cellWidth, lineHeight)
        if (c == cols && r == rows) return
        cols = c
        rows = r
        session?.resize(c, r)
        onGrid?.invoke(c, r)
    }

    override fun onDraw(canvas: Canvas) {
        // Hosted in Compose, the canvas isn't clipped to this view: drawColor
        // would paint over the header above it.
        canvas.clipRect(0, 0, width, height)
        canvas.drawColor(background)
        val f = session?.frame ?: return
        for ((row, line) in f.lines.withIndex()) {
            val top = pad + row * lineHeight
            if (top >= height) break
            for (run in line.runs) {
                if (run.bg == 0u) continue
                val left = pad + run.col.toInt() * cellWidth
                fill.color = run.bg.toInt()
                canvas.drawRect(left, top, left + run.width.toInt() * cellWidth, top + lineHeight, fill)
            }
            for (run in line.runs) drawRun(canvas, run, top, run.fg.toInt())
        }
        drawCursor(canvas, f)
    }

    private fun paintFor(run: TerminalRun): Paint = when {
        run.bold && run.italic -> boldItalic
        run.bold -> bold
        run.italic -> italic
        else -> regular
    }

    /** One run; non-ASCII text goes cell by cell so fallback glyphs stay on the grid. */
    private fun drawRun(canvas: Canvas, run: TerminalRun, top: Float, color: Int) {
        val left = pad + run.col.toInt() * cellWidth
        val text = run.text
        if (run.underline) {
            fill.color = color
            val y = top + baseline + density
            canvas.drawRect(left, y, left + run.width.toInt() * cellWidth, y + max(1f, density), fill)
        }
        if (text.isBlank()) return
        val paint = paintFor(run)
        paint.color = color
        val y = top + baseline
        if (text.all { it.code < 0x7F }) {
            canvas.drawText(text, left, y, paint)
            return
        }
        var i = 0
        var cell = 0
        while (i < text.length) {
            val n = Character.charCount(text.codePointAt(i))
            if (text[i] != ' ') canvas.drawText(text, i, i + n, left + cell * cellWidth, y, paint)
            i += n
            cell++
        }
    }

    private fun drawCursor(canvas: Canvas, f: TerminalFrame) {
        val row = f.cursorRow
        val col = f.cursorCol
        if (row < 0 || col < 0 || row >= f.lines.size) return
        val left = pad + col * cellWidth
        val top = pad + row * lineHeight
        val run = f.lines[row].runs.firstOrNull { col >= it.col.toInt() && col < it.col.toInt() + it.width.toInt() }
        val wide = run != null && run.width.toInt() == 2 && run.text.codePointCount(0, run.text.length) == 1
        val right = left + (if (wide) 2 else 1) * cellWidth
        if (!isFocused) {
            outline.color = cursorColor
            val inset = outline.strokeWidth / 2
            canvas.drawRect(left + inset, top + inset, right - inset, top + lineHeight - inset, outline)
            return
        }
        fill.color = cursorColor
        canvas.drawRect(left, top, right, top + lineHeight, fill)
        if (run == null) return
        val text = run.text
        val index = if (wide) 0 else runCatching { text.offsetByCodePoints(0, col - run.col.toInt()) }.getOrNull() ?: return
        if (index >= text.length || text[index] == ' ') return
        val paint = paintFor(run)
        paint.color = background
        canvas.drawText(text, index, index + Character.charCount(text.codePointAt(index)), left, top + baseline, paint)
    }

    override fun onFocusChanged(gainFocus: Boolean, direction: Int, previouslyFocusedRect: Rect?) {
        super.onFocusChanged(gainFocus, direction, previouslyFocusedRect)
        invalidate()
    }

    // ── gestures ───────────────────────────────────────────────────────────

    private val scroller = OverScroller(context)
    private var flingY = 0
    private var scrollRemainder = 0f
    private var selecting = false

    /** Scroll by [dy] px of finger travel (+ = down, into history). */
    private fun scrollPixels(dy: Float) {
        val s = session?.takeIf { it.isAttached } ?: return
        scrollRemainder += dy
        val lines = (scrollRemainder / lineHeight).toInt()
        if (lines != 0) {
            scrollRemainder -= lines * lineHeight
            s.screen.scroll(lines)
        }
    }

    private fun cellAt(x: Float, y: Float): Triple<UInt, UInt, Boolean> {
        val fx = ((x - pad) / cellWidth).coerceIn(0f, max(0f, cols - 0.01f))
        val row = ((y - pad) / lineHeight).toInt().coerceIn(0, max(0, rows - 1))
        val col = fx.toInt()
        return Triple(row.toUInt(), col.toUInt(), fx - col >= 0.5f)
    }

    private val gestures = GestureDetector(context, object : GestureDetector.SimpleOnGestureListener() {
        override fun onDown(e: MotionEvent): Boolean {
            scroller.forceFinished(true)
            scrollRemainder = 0f
            return true
        }

        override fun onSingleTapUp(e: MotionEvent): Boolean {
            val s = session
            if (s != null && s.isAttached && s.hasSelection) s.screen.clearSelection() else showKeyboard()
            return true
        }

        override fun onScroll(e1: MotionEvent?, e2: MotionEvent, distanceX: Float, distanceY: Float): Boolean {
            scrollPixels(-distanceY)
            return true
        }

        override fun onFling(e1: MotionEvent?, e2: MotionEvent, velocityX: Float, velocityY: Float): Boolean {
            flingY = 0
            scroller.fling(0, 0, 0, velocityY.toInt(), 0, 0, -1_000_000, 1_000_000)
            postInvalidateOnAnimation()
            return true
        }

        override fun onLongPress(e: MotionEvent) {
            val s = session?.takeIf { it.isAttached } ?: return
            val (row, col, right) = cellAt(e.x, e.y)
            s.screen.selectStart(row, col, right, TerminalSelection.WORD)
            selecting = true
            performHapticFeedback(HapticFeedbackConstants.LONG_PRESS)
        }
    })

    override fun computeScroll() {
        if (!scroller.computeScrollOffset()) return
        val y = scroller.currY
        scrollPixels((y - flingY).toFloat())
        flingY = y
        postInvalidateOnAnimation()
    }

    @SuppressLint("ClickableViewAccessibility")
    override fun onTouchEvent(event: MotionEvent): Boolean {
        if (event.actionMasked == MotionEvent.ACTION_DOWN) parent?.requestDisallowInterceptTouchEvent(true)
        if (selecting) {
            when (event.actionMasked) {
                MotionEvent.ACTION_MOVE -> session?.takeIf { it.isAttached }?.let { s ->
                    val (row, col, right) = cellAt(event.x, event.y)
                    s.screen.selectUpdate(row, col, right)
                }
                MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> selecting = false
            }
        }
        gestures.onTouchEvent(event)
        return true
    }

    // ── keyboard ───────────────────────────────────────────────────────────

    private val imm get() = context.getSystemService(InputMethodManager::class.java)

    fun showKeyboard() {
        requestFocus()
        imm?.showSoftInput(this, 0)
    }

    fun toggleKeyboard() {
        val shown = ViewCompat.getRootWindowInsets(this)?.isVisible(WindowInsetsCompat.Type.ime()) == true
        if (shown) imm?.hideSoftInputFromWindow(windowToken, 0) else showKeyboard()
    }

    override fun onCheckIsTextEditor(): Boolean = true

    override fun onCreateInputConnection(outAttrs: EditorInfo): InputConnection {
        outAttrs.inputType = InputType.TYPE_CLASS_TEXT or
            InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD or
            InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS
        outAttrs.imeOptions = EditorInfo.IME_FLAG_NO_EXTRACT_UI or
            EditorInfo.IME_FLAG_NO_FULLSCREEN or
            EditorInfo.IME_ACTION_NONE
        return Input()
    }

    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean = keyDown(keyCode, event) || super.onKeyDown(keyCode, event)

    private fun keyDown(keyCode: Int, event: KeyEvent): Boolean {
        val s = session?.takeIf { it.isAttached } ?: return false
        terminalKey(keyCode)?.let {
            s.pressKey(it, sticky, event.isCtrlPressed, event.isAltPressed, event.isShiftPressed)
            return true
        }
        val plain = event.metaState and (KeyEvent.META_CTRL_MASK or KeyEvent.META_ALT_MASK or KeyEvent.META_META_MASK).inv()
        val ch = event.getUnicodeChar(plain)
        if (ch == 0 || ch and KeyCharacterMap.COMBINING_ACCENT != 0) return false
        s.typeText(String(Character.toChars(ch)), sticky, event.isCtrlPressed, event.isAltPressed)
        return true
    }

    /**
     * The soft keyboard. Composing text is sent as it's typed and corrected
     * in place with backspaces, so a commit never sends it twice.
     */
    private inner class Input : BaseInputConnection(this@TerminalView, false) {
        /** What has been sent of the current composition. */
        private var composing = ""

        private fun replaceComposing(text: String) {
            val s = session?.takeIf { it.isAttached } ?: return
            var prefix = composing.commonPrefixWith(text).length
            if (prefix > 0 && Character.isHighSurrogate(text[prefix - 1])) prefix--
            val erase = composing.codePointCount(prefix, composing.length)
            repeat(erase) { s.screen.writeKey(TerminalKey.BACKSPACE, false, false, false) }
            s.typeText(text.substring(prefix), sticky)
            composing = text
        }

        override fun commitText(text: CharSequence, newCursorPosition: Int): Boolean {
            replaceComposing(text.toString())
            composing = ""
            return true
        }

        override fun setComposingText(text: CharSequence, newCursorPosition: Int): Boolean {
            replaceComposing(text.toString())
            return true
        }

        override fun finishComposingText(): Boolean {
            composing = ""
            return true
        }

        override fun deleteSurroundingText(beforeLength: Int, afterLength: Int): Boolean {
            val s = session?.takeIf { it.isAttached } ?: return true
            repeat(beforeLength) { s.screen.writeKey(TerminalKey.BACKSPACE, false, false, false) }
            repeat(afterLength) { s.screen.writeKey(TerminalKey.DELETE, false, false, false) }
            composing = composing.dropLast(beforeLength)
            return true
        }

        override fun deleteSurroundingTextInCodePoints(beforeLength: Int, afterLength: Int): Boolean =
            deleteSurroundingText(beforeLength, afterLength)

        override fun sendKeyEvent(event: KeyEvent): Boolean {
            if (event.action == KeyEvent.ACTION_DOWN) keyDown(event.keyCode, event)
            return true
        }

        override fun performEditorAction(actionCode: Int): Boolean {
            session?.pressKey(TerminalKey.ENTER, sticky)
            return true
        }
    }
}
