package sh.zeron.android.terminal

import android.annotation.SuppressLint
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.Rect
import android.graphics.RectF
import android.provider.Settings
import android.text.InputType
import android.view.ActionMode
import android.view.GestureDetector
import android.view.HapticFeedbackConstants
import android.view.KeyEvent
import android.view.Menu
import android.view.MenuItem
import android.view.MotionEvent
import android.view.ScaleGestureDetector
import android.view.View
import android.view.inputmethod.BaseInputConnection
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection
import android.view.inputmethod.InputMethodManager
import android.widget.OverScroller
import sh.zeron.android.core.Fonts
import sh.zeron.android.core.TerminalTab
import uniffi.zeron_core.FaceRole
import uniffi.zeron_core.KeyModifiers
import uniffi.zeron_core.TerminalCursorShape
import uniffi.zeron_core.TerminalFrame
import uniffi.zeron_core.TerminalKey
import uniffi.zeron_core.TerminalLine
import kotlin.math.floor
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt

/** Cell geometry for one font size (Geist Mono's advance, a roomy line), in pixels. */
class TerminalMetrics(val sizeDp: Float, density: Float) {
    val regular = Fonts.paint(FaceRole.MONO, sizeDp * density, ligatures = false)
    val bold = Fonts.paint(FaceRole.MONO_SEMIBOLD, sizeDp * density, ligatures = false)
    val italic = Fonts.paint(FaceRole.MONO_ITALIC, sizeDp * density, ligatures = false)
    /** The font's exact advance: runs start at `col × cellWidth`, so a rounded cell would drift every colored run. */
    val cellWidth: Float = regular.measureText("M")
    val lineHeight: Float
    /** Baseline below a row's top. */
    val ascent: Float
    val xHeight: Float

    init {
        val fm = regular.fontMetrics
        val natural = fm.descent - fm.ascent
        lineHeight = (natural * 1.22f).roundToInt().toFloat()
        ascent = -fm.ascent + (lineHeight - natural) / 2
        val bounds = Rect()
        regular.getTextBounds("x", 0, 1, bounds)
        xHeight = bounds.height().toFloat()
    }
}

/**
 * A terminal grid on screen (iOS `TerminalView`). The emulator lives in Rust
 * (`TerminalSession`); this paints its frames on a monospace grid and turns
 * touches and keys into input. Scrollback flings natively and glides by the
 * pixel (the core keeps the fraction of a row); a full-screen program gets
 * drags as its wheel and taps as clicks when it tracks the pointer.
 */
@SuppressLint("ViewConstructor")
class TerminalView(context: Context, val tab: TerminalTab) : View(context) {
    private val session get() = tab.session
    private val density = resources.displayMetrics.density
    private val prefs = context.getSharedPreferences("terminal", 0)

    var frameModel: TerminalFrame? = null
        private set
    /** Sticky modifiers from the key bar, applied to the next key. */
    var modifiers: () -> KeyModifiers = { NO_MODS }
    var consumeModifiers: () -> Unit = {}
    /** Selected text to put in the chat's draft. */
    var onAddToMessage: ((String) -> Unit)? = null
    var onFocusChanged: ((Boolean) -> Unit)? = null
    /** First frame with something painted (the screen's loader steps aside). */
    var onContent: (() -> Unit)? = null

    /** Default foreground, cursor/selection accent (ARGB). */
    var foreground: Int = Color.BLACK
        set(value) { field = value; invalidate() }
    var accent: Int = Color.BLUE
        set(value) { field = value; invalidate() }

    var fontSize: Float = prefs.getFloat("fontSize", DEFAULT_SIZE)
        private set
    private var metrics = TerminalMetrics(fontSize, density)
    private val insetLeft = 10 * density
    private val insetTop = 6 * density
    private val insetRight = 8 * density
    private val insetBottom = 6 * density

    private var unsubscribe: (() -> Unit)? = null

    init {
        isFocusable = true
        isFocusableInTouchMode = true
        contentDescription = "Terminal"
        tab.onBell = { performHapticFeedback(HapticFeedbackConstants.KEYBOARD_TAP) }
    }

    override fun onAttachedToWindow() {
        super.onAttachedToWindow()
        unsubscribe = tab.observe { refresh() }
        refresh()
    }

    override fun onDetachedFromWindow() {
        super.onDetachedFromWindow()
        unsubscribe?.invoke()
        unsubscribe = null
        removeCallbacks(blink)
        removeCallbacks(resizeWork)
        actionMode?.finish()
    }

    // ── Frames ───────────────────────────────────────────────────────────

    private var hadContent = false

    private fun refresh() {
        val frame = session.frame()
        frameModel = frame
        if (!hadContent && frame.lines.any { it.runs.isNotEmpty() }) {
            hadContent = true
            onContent?.invoke()
        }
        if (!touching && scroller.isFinished) {
            // The core's position wins (typing follows the live bottom).
            back = if (frame.altScreen) 0f else (frame.displayOffset.toFloat() + frame.scrollFrac) * metrics.lineHeight
        }
        restartBlink()
        invalidate()
    }

    val hasContent: Boolean get() = hadContent

    // ── Grid size ────────────────────────────────────────────────────────

    val grid: Pair<Int, Int>
        get() {
            val w = width - insetLeft - insetRight
            val h = height - insetTop - insetBottom
            return max(20, floor(w / metrics.cellWidth).toInt()) to max(4, floor(h / metrics.lineHeight).toInt())
        }

    private var lastGrid: Pair<Int, Int>? = null
    private val resizeWork = Runnable {
        val g = grid
        lastGrid = g
        session.resize(g.first.toUShort(), g.second.toUShort())
    }

    override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
        super.onSizeChanged(w, h, oldw, oldh)
        scheduleResize()
    }

    /** The host follows the view's size once it settles (the keyboard sliding in mustn't resize the PTY a dozen times). */
    private fun scheduleResize() {
        if (width < 40 || height < 40) return
        if (lastGrid == grid) return
        removeCallbacks(resizeWork)
        postDelayed(resizeWork, if (lastGrid == null) 0 else 80)
    }

    // ── Scrollback ───────────────────────────────────────────────────────

    /** How far back from the live bottom the grid shows, in pixels. */
    private var back = 0f
    private val scroller = OverScroller(context)
    private var touching = false

    private val maxBack: Float
        get() = frameModel?.let { if (it.altScreen) 0f else it.history.toFloat() * metrics.lineHeight } ?: 0f

    /** Programs that own the wheel: full-screen ones, or ones tracking the pointer. */
    private val programWheel: Boolean get() = frameModel?.let { it.altScreen || it.mouse } == true

    private fun scrollTo(px: Float) {
        val clamped = px.coerceIn(0f, maxBack)
        if (clamped == back) return
        back = clamped
        session.scrollToPosition(back / metrics.lineHeight)
    }

    override fun computeScroll() {
        if (scroller.computeScrollOffset()) {
            scrollTo(scroller.currY.toFloat())
            postInvalidateOnAnimation()
        }
    }

    private var wheelRemainder = 0f

    // ── Gestures ─────────────────────────────────────────────────────────

    private var selecting = false

    private val gestures = GestureDetector(context, object : GestureDetector.SimpleOnGestureListener() {
        override fun onDown(e: MotionEvent): Boolean {
            if (!scroller.isFinished) scroller.forceFinished(true)
            wheelRemainder = 0f
            return true
        }

        override fun onSingleTapUp(e: MotionEvent): Boolean {
            if (session.selection() != null) {
                session.clearSelection()
                actionMode?.finish()
                return true
            }
            // A program tracking the pointer (htop, vim with the mouse on) gets the tap as a click.
            if (frameModel?.mouse == true) {
                val (row, col) = cell(e.x, e.y)
                session.click(row.toUShort(), col.toUShort())
            }
            showKeyboard()
            return true
        }

        override fun onScroll(e1: MotionEvent?, e2: MotionEvent, dx: Float, dy: Float): Boolean {
            if (selecting || scaling) return true
            if (programWheel) {
                // Rows as a fraction: the core owes the program whole ones.
                val (row, col) = cell(e2.x, e2.y)
                session.wheel(-dy / metrics.lineHeight, row.toUShort(), col.toUShort())
            } else {
                scrollTo(back - dy)
            }
            return true
        }

        override fun onFling(e1: MotionEvent?, e2: MotionEvent, vx: Float, vy: Float): Boolean {
            if (selecting || scaling || programWheel) return true
            scroller.fling(0, back.toInt(), 0, vy.toInt(), 0, 0, 0, maxBack.toInt())
            postInvalidateOnAnimation()
            return true
        }

        override fun onLongPress(e: MotionEvent) {
            if (scaling) return
            selecting = true
            actionMode?.finish()
            performHapticFeedback(HapticFeedbackConstants.LONG_PRESS)
            val (row, col) = cell(e.x, e.y)
            session.select(row.toUShort(), col.toUShort(), true)
        }
    }).apply { setIsLongpressEnabled(true) }

    private var scaling = false
    private var pinchBase = fontSize

    private val pinch = ScaleGestureDetector(context, object : ScaleGestureDetector.SimpleOnScaleGestureListener() {
        override fun onScaleBegin(detector: ScaleGestureDetector): Boolean {
            scaling = true
            pinchBase = fontSize
            return true
        }

        override fun onScale(detector: ScaleGestureDetector): Boolean {
            pinchBase *= detector.scaleFactor
            val size = ((pinchBase.coerceIn(MIN_SIZE, MAX_SIZE)) * 2).roundToInt() / 2f
            if (size != fontSize) {
                performHapticFeedback(if (size == MIN_SIZE || size == MAX_SIZE) HapticFeedbackConstants.LONG_PRESS else HapticFeedbackConstants.CLOCK_TICK)
                setFontSize(size)
            }
            return true
        }
    })

    @SuppressLint("ClickableViewAccessibility")
    override fun onTouchEvent(event: MotionEvent): Boolean {
        pinch.onTouchEvent(event)
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                touching = true
                parent?.requestDisallowInterceptTouchEvent(true)
            }
            MotionEvent.ACTION_MOVE -> if (selecting) {
                val (row, col) = cell(event.x, event.y)
                session.extendSelection(row.toUShort(), col.toUShort())
            }
            MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> {
                touching = false
                if (selecting) {
                    selecting = false
                    if (event.actionMasked == MotionEvent.ACTION_UP) showSelectionMenu()
                }
                if (event.pointerCount <= 1) scaling = false
            }
        }
        if (!scaling) gestures.onTouchEvent(event)
        return true
    }

    private fun cell(x: Float, y: Float): Pair<Int, Int> {
        val f = frameModel
        val cols = f?.cols?.toInt() ?: 80
        val rows = f?.rows?.toInt() ?: 24
        val col = max(0f, (x - insetLeft) / metrics.cellWidth).toInt()
        val row = max(0f, (y - insetTop) / metrics.lineHeight).toInt()
        return min(row, rows - 1) to min(col, cols - 1)
    }

    fun setFontSize(size: Float) {
        fontSize = size
        prefs.edit().putFloat("fontSize", size).apply()
        metrics = TerminalMetrics(size, density)
        scheduleResize()
        invalidate()
    }

    // ── Selection menu ───────────────────────────────────────────────────

    private var actionMode: ActionMode? = null
    /** 1 at the moment of a copy (a brighter selection wash). */
    private var selectionBoost = 0f

    private fun showSelectionMenu() {
        val selection = session.selection() ?: return
        val x0 = insetLeft + min(selection.startCol.toInt(), selection.endCol.toInt()) * metrics.cellWidth
        val x1 = insetLeft + (max(selection.startCol.toInt(), selection.endCol.toInt()) + 1) * metrics.cellWidth
        val y0 = insetTop + selection.startRow.toFloat() * metrics.lineHeight
        val y1 = insetTop + (selection.endRow.toFloat() + 1) * metrics.lineHeight
        actionMode = startActionMode(object : ActionMode.Callback2() {
            override fun onCreateActionMode(mode: ActionMode, menu: Menu): Boolean {
                menu.add(0, 1, 0, "Copy")
                if (onAddToMessage != null) menu.add(0, 2, 1, "Add to message")
                menu.add(0, 3, 2, "Paste")
                return true
            }

            override fun onPrepareActionMode(mode: ActionMode, menu: Menu) = false

            override fun onActionItemClicked(mode: ActionMode, item: MenuItem): Boolean {
                val text = session.selection()?.text ?: return false.also { mode.finish() }
                when (item.itemId) {
                    1 -> {
                        clipboard.setPrimaryClip(ClipData.newPlainText("Terminal", text))
                        flashSelection()
                    }
                    2 -> {
                        onAddToMessage?.invoke(text)
                        session.clearSelection()
                    }
                    3 -> {
                        session.clearSelection()
                        pasteFromClipboard()
                    }
                }
                mode.finish()
                return true
            }

            override fun onDestroyActionMode(mode: ActionMode) {
                actionMode = null
            }

            override fun onGetContentRect(mode: ActionMode, view: View, outRect: Rect) {
                outRect.set(x0.toInt(), y0.toInt(), x1.toInt(), y1.toInt())
            }
        }, ActionMode.TYPE_FLOATING)
    }

    /** The copy flash (the desktop's): the selection brightens, then lets go. */
    private fun flashSelection() {
        performHapticFeedback(HapticFeedbackConstants.CONFIRM)
        selectionBoost = 1f
        invalidate()
        postDelayed({
            selectionBoost = 0f
            session.clearSelection()
        }, 180)
    }

    private val clipboard get() = context.getSystemService(ClipboardManager::class.java)

    // ── Keyboard ─────────────────────────────────────────────────────────

    private val imm get() = context.getSystemService(InputMethodManager::class.java)

    fun showKeyboard() {
        if (!hasFocus()) requestFocus()
        imm.showSoftInput(this, InputMethodManager.SHOW_IMPLICIT)
    }

    fun hideKeyboard() {
        imm.hideSoftInputFromWindow(windowToken, 0)
        clearFocus()
    }

    override fun onFocusChanged(gainFocus: Boolean, direction: Int, previouslyFocusedRect: Rect?) {
        super.onFocusChanged(gainFocus, direction, previouslyFocusedRect)
        session.setFocused(gainFocus)
        typed()
        onFocusChanged?.invoke(gainFocus)
        invalidate()
    }

    override fun onCheckIsTextEditor() = true

    override fun onCreateInputConnection(outAttrs: EditorInfo): InputConnection {
        // Plain text, no suggestions or autocorrect: a terminal wants exactly what was typed.
        outAttrs.inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD or InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS
        outAttrs.imeOptions = EditorInfo.IME_FLAG_NO_FULLSCREEN or EditorInfo.IME_FLAG_NO_EXTRACT_UI or EditorInfo.IME_ACTION_NONE
        return object : BaseInputConnection(this, true) {
            override fun commitText(text: CharSequence?, newCursorPosition: Int): Boolean {
                super.commitText(text, newCursorPosition)
                flush()
                return true
            }

            override fun finishComposingText(): Boolean {
                super.finishComposingText()
                flush()
                return true
            }

            override fun deleteSurroundingText(beforeLength: Int, afterLength: Int): Boolean {
                repeat(beforeLength) { press(TerminalKey.Backspace) }
                return super.deleteSurroundingText(beforeLength, afterLength)
            }

            private fun flush() {
                val content = editable ?: return
                val text = content.toString()
                content.clear()
                if (text.isNotEmpty()) insertText(text)
            }
        }
    }

    /** Typed text: newlines are Enter, everything else goes as typed. */
    fun insertText(text: String) {
        typed()
        val parts = text.split('\n')
        parts.forEachIndexed { i, part ->
            if (part.isNotEmpty()) session.typeText(part, modifiers())
            if (i < parts.size - 1) session.press(TerminalKey.Enter, NO_MODS)
        }
        consumeModifiers()
    }

    fun press(key: TerminalKey) {
        typed()
        session.press(key, modifiers())
        consumeModifiers()
    }

    fun pasteFromClipboard() {
        val text = clipboard.primaryClip?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.coerceToText(context)?.toString()
        if (!text.isNullOrEmpty()) session.paste(text)
    }

    /** Hardware keys (and the soft keyboard's Delete/Enter key events). */
    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean {
        val bar = modifiers()
        val mods = KeyModifiers(
            ctrl = event.isCtrlPressed || bar.ctrl,
            alt = event.isAltPressed || bar.alt,
            shift = event.isShiftPressed,
        )
        val named: TerminalKey? = when (keyCode) {
            KeyEvent.KEYCODE_DPAD_UP -> TerminalKey.Up
            KeyEvent.KEYCODE_DPAD_DOWN -> TerminalKey.Down
            KeyEvent.KEYCODE_DPAD_LEFT -> TerminalKey.Left
            KeyEvent.KEYCODE_DPAD_RIGHT -> TerminalKey.Right
            KeyEvent.KEYCODE_ESCAPE -> TerminalKey.Escape
            KeyEvent.KEYCODE_MOVE_HOME -> TerminalKey.Home
            KeyEvent.KEYCODE_MOVE_END -> TerminalKey.End
            KeyEvent.KEYCODE_PAGE_UP -> TerminalKey.PageUp
            KeyEvent.KEYCODE_PAGE_DOWN -> TerminalKey.PageDown
            KeyEvent.KEYCODE_FORWARD_DEL -> TerminalKey.Delete
            KeyEvent.KEYCODE_INSERT -> TerminalKey.Insert
            KeyEvent.KEYCODE_TAB -> TerminalKey.Tab
            KeyEvent.KEYCODE_ENTER, KeyEvent.KEYCODE_NUMPAD_ENTER -> TerminalKey.Enter
            KeyEvent.KEYCODE_DEL -> TerminalKey.Backspace
            in KeyEvent.KEYCODE_F1..KeyEvent.KEYCODE_F12 -> TerminalKey.Function((keyCode - KeyEvent.KEYCODE_F1 + 1).toUByte())
            else -> null
        }
        typed()
        if (named != null) {
            session.press(named, mods)
            consumeModifiers()
            return true
        }
        // Ctrl/Alt combos fold to the character without them.
        val ch = event.getUnicodeChar(if (mods.ctrl || mods.alt) 0 else event.metaState)
        if (ch == 0 || event.isMetaPressed) return super.onKeyDown(keyCode, event)
        session.typeText(String(Character.toChars(ch)), mods.copy(shift = false))
        consumeModifiers()
        return true
    }

    // ── Cursor blink ─────────────────────────────────────────────────────

    private var lastKey = 0L
    private var cursorOn = true
    private val animations get() = Settings.Global.getFloat(context.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f) > 0f

    /** The blink rests a while after the last key (an idle terminal schedules no frames). */
    private val blinking: Boolean
        get() = hasFocus() && frameModel?.cursorBlinks == true && animations && System.currentTimeMillis() - lastKey < BLINK_IDLE_MS

    private val blink: Runnable = object : Runnable {
        override fun run() {
            if (!blinking) {
                cursorOn = true
                invalidate()
                return
            }
            cursorOn = !cursorOn
            invalidate()
            postDelayed(this, if (cursorOn) 550 else 450)
        }
    }

    private fun typed() {
        lastKey = System.currentTimeMillis()
        restartBlink()
    }

    /** Typing restarts the blink in its visible phase. */
    private fun restartBlink() {
        removeCallbacks(blink)
        cursorOn = true
        if (blinking) postDelayed(blink, 550)
    }

    // ── Painting ─────────────────────────────────────────────────────────

    private val fill = Paint()
    private val cursorPaint = Paint(Paint.ANTI_ALIAS_FLAG)
    private val cell = RectF()

    override fun onDraw(canvas: Canvas) {
        val frame = frameModel ?: return
        val m = metrics
        val selection = withAlpha(accent, 0.28f + 0.3f * selectionBoost)
        // A partial scroll: rows sit `dy` lower, the revealed row above them.
        val dy = frame.scrollFrac * m.lineHeight
        val rows = ArrayList<Pair<Int, TerminalLine>>(frame.lines.size + 1)
        frame.above?.let { rows.add(-1 to it) }
        frame.lines.forEachIndexed { i, line -> rows.add(i to line) }
        canvas.save()
        canvas.clipRect(0f, 0f, width.toFloat(), height.toFloat())
        // Backgrounds and the selection wash.
        for ((r, line) in rows) {
            val y = insetTop + r * m.lineHeight + dy
            for (run in line.runs) {
                val x = insetLeft + run.col.toFloat() * m.cellWidth
                val w = run.cells.toFloat() * m.cellWidth
                if (run.bg != 0u) {
                    fill.color = argb(run.bg)
                    canvas.drawRect(x, y, x + w, y + m.lineHeight, fill)
                }
                if (run.selected) {
                    fill.color = selection
                    canvas.drawRect(x, y, x + w, y + m.lineHeight, fill)
                }
            }
        }
        // Text, with box drawing and blocks painted into their cells.
        for ((r, line) in rows) {
            val top = insetTop + r * m.lineHeight + dy
            val baseline = top + m.ascent
            for (run in line.runs) {
                if (run.text.isBlank()) continue
                val color = if (run.fg == 0u) foreground else argb(run.fg)
                val paint = if (run.bold) m.bold else if (run.italic) m.italic else m.regular
                paint.color = color
                paint.isUnderlineText = run.underline
                var x = insetLeft + run.col.toFloat() * m.cellWidth
                val span = (if (run.wide) 2 else 1) * m.cellWidth
                val ascii = run.text.all { it.code < 0x80 }
                if (ascii && !run.wide) {
                    canvas.drawText(run.text, x, baseline, paint)
                } else {
                    // Fallback glyphs (CJK, symbols, emoji) don't share the mono
                    // advance: each sits on its own cell(s), centered.
                    var i = 0
                    val text = run.text
                    while (i < text.length) {
                        val cp = text.codePointAt(i)
                        val n = Character.charCount(cp)
                        if (TerminalGlyphs.draws(cp)) {
                            cell.set(x, top, x + span, top + m.lineHeight)
                            TerminalGlyphs.draw(cp, cell, baseline, m.xHeight, color, canvas)
                        } else if (cp != ' '.code) {
                            val w = paint.measureText(text, i, i + n)
                            canvas.drawText(text, i, i + n, x + max(0f, (span - w) / 2), baseline, paint)
                        }
                        x += span
                        i += n
                    }
                }
                paint.isUnderlineText = false
                // Typed ahead of the host's echo: a faint underline owns that it's provisional.
                if (run.predicted) {
                    fill.color = withAlpha(color, 0.45f)
                    val ux = insetLeft + run.col.toFloat() * m.cellWidth
                    canvas.drawRect(ux, baseline + density, ux + run.cells.toFloat() * m.cellWidth, baseline + 2 * density, fill)
                }
            }
        }
        canvas.restore()
        drawCursor(canvas, frame)
    }

    private fun drawCursor(canvas: Canvas, frame: TerminalFrame) {
        val c = frame.cursor ?: return
        if (frame.displayOffset != 0u) return
        if (blinking && !cursorOn) return
        val m = metrics
        val x = insetLeft + c.col.toFloat() * m.cellWidth
        val y = insetTop + c.row.toFloat() * m.lineHeight
        val focused = hasFocus()
        cursorPaint.style = Paint.Style.FILL
        cursorPaint.color = accent
        when (c.shape) {
            TerminalCursorShape.BEAM -> canvas.drawRect(x, y + density, x + 2 * density, y + m.lineHeight - density, cursorPaint)
            TerminalCursorShape.UNDERLINE -> canvas.drawRect(x, y + m.lineHeight - 2 * density, x + m.cellWidth, y + m.lineHeight, cursorPaint)
            TerminalCursorShape.BLOCK, TerminalCursorShape.HOLLOW_BLOCK -> {
                if (c.shape == TerminalCursorShape.HOLLOW_BLOCK || !focused) {
                    cursorPaint.style = Paint.Style.STROKE
                    cursorPaint.strokeWidth = density
                    cursorPaint.color = withAlpha(accent, 0.8f)
                    canvas.drawRoundRect(x + density / 2, y + density * 1.5f, x + m.cellWidth - density / 2, y + m.lineHeight - density * 1.5f, density, density, cursorPaint)
                } else {
                    cursorPaint.color = withAlpha(accent, 0.55f)
                    canvas.drawRoundRect(x, y + density, x + m.cellWidth, y + m.lineHeight - density, density, density, cursorPaint)
                }
            }
        }
    }

    companion object {
        val NO_MODS = KeyModifiers(ctrl = false, alt = false, shift = false)
        const val DEFAULT_SIZE = 12f
        const val MIN_SIZE = 8f
        const val MAX_SIZE = 22f
        private const val BLINK_IDLE_MS = 15_000L

        /** `0xRRGGBBAA` → ARGB. */
        fun argb(rgba: UInt): Int = ((rgba and 0xFFu) shl 24 or (rgba shr 8)).toInt()

        fun withAlpha(color: Int, alpha: Float): Int =
            (((color ushr 24) * alpha).roundToInt().coerceIn(0, 255) shl 24) or (color and 0xFFFFFF)

        /** A rough grid before the view has a size (the shell opens at it, then follows). */
        fun guessGrid(widthPx: Int, heightPx: Int, density: Float): Pair<Int, Int> {
            val m = TerminalMetrics(DEFAULT_SIZE, density)
            val w = max(320 * density, widthPx.toFloat()) - 18 * density
            val h = max(400 * density, heightPx * 0.55f)
            return max(20, floor(w / m.cellWidth).toInt()) to max(8, floor(h / m.lineHeight).toInt())
        }
    }
}
