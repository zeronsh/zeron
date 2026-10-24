package sh.zeron.android.transcript

import android.graphics.Bitmap
import android.graphics.Canvas as BitmapCanvas
import android.graphics.DashPathEffect
import android.graphics.Paint
import android.graphics.RectF
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.drawscope.drawIntoCanvas
import androidx.compose.ui.graphics.nativeCanvas
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.semantics.ProgressBarRangeInfo
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.progressBarRangeInfo
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.setProgress
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.unit.dp
import androidx.compose.ui.zIndex
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import sh.zeron.android.design.LocalDarkTheme
import uniffi.zeron_core.ControlKind
import uniffi.zeron_core.FrameImage
import uniffi.zeron_core.PointerPhase
import uniffi.zeron_core.Rgba
import uniffi.zeron_core.SurfaceFrame
import uniffi.zeron_core.SurfaceGesture
import uniffi.zeron_core.VisualControl
import uniffi.zeron_core.VisualDisplay
import uniffi.zeron_core.VisualListener
import uniffi.zeron_core.VisualPaint
import uniffi.zeron_core.VisualSession
import uniffi.zeron_core.VisualShape
import java.nio.ByteBuffer
import kotlin.math.ceil
import kotlin.math.max
import kotlin.math.min

/**
 * An agent visual in the transcript. Its Rust worker ([VisualSession]) runs
 * the script, lays it out, draws its canvases and charts and renders its
 * scenes and shaders; this only shows what arrives and forwards touches.
 * Paint is rasterized on the worker's thread, so the main thread just swaps
 * bitmaps. Frames flow while it's composed and the app is in the foreground.
 */
@Composable
fun TranscriptVisual(session: VisualSession, openUrl: (String) -> Unit, modifier: Modifier = Modifier) {
    val density = LocalDensity.current.density
    val dark = LocalDarkTheme.current
    val context = LocalContext.current
    val sink = remember(session, density) { VisualSink(density) }
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    val darkNow by rememberUpdatedState(dark)

    DisposableEffect(session, sink, lifecycle) {
        val reduceMotion = Settings.Global.getFloat(context.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f
        var attached = false
        fun sync() {
            val wanted = lifecycle.currentState.isAtLeast(Lifecycle.State.STARTED)
            if (wanted == attached) return
            attached = wanted
            if (wanted) session.attach(sink, darkNow, reduceMotion) else session.detach()
        }
        val observer = LifecycleEventObserver { _, _ -> sync() }
        lifecycle.addObserver(observer)
        sync()
        onDispose {
            lifecycle.removeObserver(observer)
            if (attached) session.detach()
        }
    }
    // Attach carries the appearance; later flips go through setAppearance.
    var shownDark by remember(session) { mutableStateOf(dark) }
    LaunchedEffect(session, dark) {
        if (dark != shownDark) {
            shownDark = dark
            session.setAppearance(dark)
        }
    }

    val shown = sink.shown
    Box(modifier.fillMaxSize().semantics { contentDescription = session.title() }) {
        shown?.base?.let { base ->
            Canvas(Modifier.size(shown.display.width.dp, shown.display.height.dp)) {
                drawIntoCanvas { it.nativeCanvas.drawBitmap(base, null, RectF(0f, 0f, size.width, size.height), bitmapPaint) }
            }
        }
        shown?.display?.surfaces?.forEach { s ->
            VisualSurfaceLayer(session, s.id, s.gesture, sink.frames[s.id], Modifier.offset(s.x.dp, s.y.dp).size(max(s.w, 0f).dp, max(s.h, 0f).dp))
        }
        // Later controls above earlier ones, as laid out (a tappable stack under the controls inside it).
        shown?.display?.controls?.forEachIndexed { i, c ->
            Control(session, c, openUrl, Modifier.zIndex(1f + i))
        }
    }
}

/**
 * A `WidgetKind.Visual` slot: its session, looked up once it exists (the
 * worker spawns when the row is first laid out) and kept across row versions.
 */
@Composable
fun VisualWidget(state: TranscriptState, model: RowModel, id: String, openUrl: (String) -> Unit) {
    var session by remember(id) { mutableStateOf(state.engine.visual(id)) }
    if (session == null) LaunchedEffect(id, model) { session = state.engine.visual(id) }
    session?.let { TranscriptVisual(it, openUrl) }
}

private val bitmapPaint = Paint(Paint.FILTER_BITMAP_FLAG)

/** A canvas, chart, scene or shader: the worker's image under its vector paint. */
@Composable
private fun VisualSurfaceLayer(session: VisualSession, id: ULong, gesture: SurfaceGesture, picture: SurfacePicture?, modifier: Modifier) {
    val density = LocalDensity.current.density
    val scope = rememberCoroutineScope()
    var readout by remember { mutableStateOf<Job?>(null) }
    fun send(phase: PointerPhase, x: Float, y: Float) = session.pointer(id, phase, x / density, y / density)
    val input = when (gesture) {
        SurfaceGesture.NONE -> Modifier
        SurfaceGesture.TAP -> Modifier.pointerInput(id) {
            detectTapGestures { p ->
                send(PointerPhase.DOWN, p.x, p.y)
                send(PointerPhase.UP, p.x, p.y)
            }
        }
        // Tracks from touch-down and keeps every move, so drags here never scroll the transcript.
        SurfaceGesture.DRAG -> Modifier.pointerInput(id) {
            awaitEachGesture {
                val down = awaitFirstDown()
                down.consume()
                send(PointerPhase.DOWN, down.position.x, down.position.y)
                var last = down.position
                while (true) {
                    val event = awaitPointerEvent()
                    val change = event.changes.firstOrNull { it.id == down.id }
                    if (change == null) {
                        send(PointerPhase.CANCEL, last.x, last.y)
                        break
                    }
                    last = change.position
                    if (!change.pressed) {
                        send(PointerPhase.UP, last.x, last.y)
                        change.consume()
                        break
                    }
                    if (change.isConsumed) {
                        send(PointerPhase.CANCEL, last.x, last.y)
                        break
                    }
                    change.consume()
                    send(PointerPhase.MOVE, last.x, last.y)
                }
            }
        }
        // Mostly-horizontal drags scrub; vertical ones scroll the transcript. A tap shows the readout a moment.
        SurfaceGesture.HORIZONTAL_DRAG -> Modifier
            .pointerInput(id) {
                detectTapGestures { p ->
                    readout?.cancel()
                    send(PointerPhase.DOWN, p.x, p.y)
                    readout = scope.launch {
                        delay(2500)
                        send(PointerPhase.UP, p.x, p.y)
                    }
                }
            }
            .pointerInput(id) {
                var last = androidx.compose.ui.geometry.Offset.Zero
                detectHorizontalDragGestures(
                    onDragStart = { p ->
                        readout?.cancel()
                        last = p
                        send(PointerPhase.DOWN, p.x, p.y)
                    },
                    onDragEnd = { send(PointerPhase.UP, last.x, last.y) },
                    onDragCancel = { send(PointerPhase.CANCEL, last.x, last.y) },
                    onHorizontalDrag = { change, _ ->
                        change.consume()
                        last = change.position
                        send(PointerPhase.MOVE, last.x, last.y)
                    },
                )
            }
    }
    Canvas(modifier.then(input)) {
        val dst = RectF(0f, 0f, size.width, size.height)
        drawIntoCanvas { c ->
            picture?.image?.let { c.nativeCanvas.drawBitmap(it, null, dst, bitmapPaint) }
            picture?.paint?.let { c.nativeCanvas.drawBitmap(it, null, dst, bitmapPaint) }
        }
    }
}

/** A touch target over control art the worker painted: buttons, tabs, toggles, sliders, links, tappable stacks. */
@Composable
private fun Control(session: VisualSession, c: VisualControl, openUrl: (String) -> Unit, modifier: Modifier) {
    val haptics = LocalHapticFeedback.current
    val kind = c.kind
    // A finger-sized target around small art.
    val padX = 4f
    val padY = max(0f, (44f - c.h) / 2)
    val radius = when (kind) {
        is ControlKind.Tap -> 6.dp
        is ControlKind.Button, is ControlKind.Tab -> 8.dp
        else -> 0.dp
    }
    val interaction = remember { MutableInteractionSource() }
    val pressed by interaction.collectIsPressedAsState()
    val wash = MaterialTheme.colorScheme.onSurface.copy(alpha = 0.08f)
    val density = LocalDensity.current.density
    val box = modifier
        .offset((c.x - padX).dp, (c.y - padY).dp)
        .size((max(c.w, 0f) + padX * 2).dp, (max(c.h, 0f) + padY * 2).dp)
        .semantics {
            contentDescription = c.label
            when (kind) {
                is ControlKind.Button, is ControlKind.Tap -> role = Role.Button
                is ControlKind.Tab -> {
                    role = Role.Tab
                    selected = kind.selected
                }
                is ControlKind.Toggle -> {
                    role = Role.Switch
                    stateDescription = if (kind.on) "On" else "Off"
                }
                is ControlKind.Slider -> {
                    stateDescription = kind.shown
                    progressBarRangeInfo = ProgressBarRangeInfo(kind.fraction, 0f..1f)
                    setProgress { v ->
                        session.slide(c.id, v.coerceIn(0f, 1f))
                        true
                    }
                }
                is ControlKind.Link -> Unit
            }
        }
    when (kind) {
        is ControlKind.Slider -> Box(
            box.pointerInput(c.id, kind.trackX, kind.trackW) {
                // x in the visual's coordinates: the box starts padX left of the art.
                fun slide(px: Float) {
                    if (kind.trackW <= 0f) return
                    val x = c.x - padX + px / density
                    session.slide(c.id, ((x - kind.trackX) / kind.trackW).coerceIn(0f, 1f))
                }
                awaitEachGesture {
                    val down = awaitFirstDown()
                    down.consume()
                    slide(down.position.x)
                    while (true) {
                        val event = awaitPointerEvent()
                        val change = event.changes.firstOrNull { it.id == down.id } ?: break
                        if (!change.pressed) break
                        change.consume()
                        slide(change.position.x)
                    }
                }
            },
        )
        else -> Box(
            box.clickable(interactionSource = interaction, indication = null) {
                when (kind) {
                    is ControlKind.Link -> openUrl(kind.url)
                    is ControlKind.Tab, is ControlKind.Toggle -> {
                        haptics.performHapticFeedback(HapticFeedbackType.SegmentTick)
                        session.activate(c.id)
                    }
                    else -> session.activate(c.id)
                }
            },
        ) {
            if (pressed && (kind is ControlKind.Button || kind is ControlKind.Tap || kind is ControlKind.Tab || kind is ControlKind.Link)) {
                Box(
                    Modifier
                        .padding(horizontal = padX.dp, vertical = padY.dp)
                        .fillMaxSize()
                        .clip(RoundedCornerShape(radius))
                        .background(wash),
                )
            }
        }
    }
}

/** A surface's latest picture: the worker's image, then its vector paint over it. */
private class SurfacePicture(val image: Bitmap?, val paint: Bitmap?)

/** A display and its static paint, rasterized. */
private class Shown(val display: VisualDisplay, val base: Bitmap?)

/**
 * Receives the worker's output on its thread, rasterizes it there, and hands
 * the main thread only the latest of each.
 */
private class VisualSink(private val density: Float) : VisualListener {
    private val main = Handler(Looper.getMainLooper())
    /** Worker thread only (paint colors mutate per run). */
    private val fonts = StyleFonts()
    private val lock = Any()
    private var sizes: Map<ULong, Pair<Float, Float>> = emptyMap()
    private var pendingDisplay: Shown? = null
    private val pendingFrames = HashMap<ULong, SurfacePicture>()
    private var scheduled = false

    var shown by mutableStateOf<Shown?>(null)
        private set
    val frames = mutableStateMapOf<ULong, SurfacePicture>()

    override fun display(display: VisualDisplay) {
        fonts.update(display.styles, density)
        val base = VisualPainter.render(display.paint, display.width, display.height, density, fonts)
        synchronized(lock) {
            sizes = display.surfaces.associate { it.id to (it.w to it.h) }
            pendingDisplay = Shown(display, base)
            schedule()
        }
    }

    override fun frame(frame: SurfaceFrame) {
        fonts.update(frame.styles, density)
        val size = synchronized(lock) { sizes[frame.surface] } ?: return
        val image = frame.image?.let(VisualPainter::image)
        val paint = if (frame.paint.shapes.isEmpty() && frame.paint.runs.isEmpty()) null
        else VisualPainter.render(frame.paint, size.first, size.second, density, fonts)
        synchronized(lock) {
            pendingFrames[frame.surface] = SurfacePicture(image, paint)
            schedule()
        }
    }

    /** Called with the lock held. */
    private fun schedule() {
        if (scheduled) return
        scheduled = true
        main.post {
            val display: Shown?
            val latest: Map<ULong, SurfacePicture>
            synchronized(lock) {
                display = pendingDisplay
                latest = HashMap(pendingFrames)
                pendingDisplay = null
                pendingFrames.clear()
                scheduled = false
            }
            if (display != null) {
                shown = display
                val live = display.display.surfaces.mapTo(HashSet()) { it.id }
                frames.keys.retainAll(live)
            }
            frames.putAll(latest)
        }
    }
}

/** Draws Rust-placed shapes and runs into bitmaps. Safe off the main thread. */
object VisualPainter {
    fun color(c: Rgba): Int = android.graphics.Color.argb(c.a.coerceIn(0f, 1f), c.r.coerceIn(0f, 1f), c.g.coerceIn(0f, 1f), c.b.coerceIn(0f, 1f))

    /**
     * A worker frame (premultiplied BGRA) as a bitmap. ARGB_8888 holds its
     * bytes as RGBA, so red and blue swap first.
     */
    fun image(frame: FrameImage): Bitmap? {
        val w = frame.width.toInt()
        val h = frame.height.toInt()
        val bytes = frame.bgra
        if (w <= 0 || h <= 0 || bytes.size < w * h * 4) return null
        var i = 0
        val n = w * h * 4
        while (i < n) {
            val b = bytes[i]
            bytes[i] = bytes[i + 2]
            bytes[i + 2] = b
            i += 4
        }
        return Bitmap.createBitmap(w, h, Bitmap.Config.ARGB_8888).apply {
            copyPixelsFromBuffer(ByteBuffer.wrap(bytes, 0, n))
        }
    }

    internal fun render(paint: VisualPaint, width: Float, height: Float, density: Float, fonts: StyleFonts): Bitmap? {
        val w = ceil(width * density).toInt()
        val h = ceil(height * density).toInt()
        if (w <= 0 || h <= 0) return null
        if (paint.shapes.isEmpty() && paint.runs.isEmpty()) return null
        val bitmap = Bitmap.createBitmap(w, h, Bitmap.Config.ARGB_8888)
        draw(paint, BitmapCanvas(bitmap), density, fonts)
        return bitmap
    }

    internal fun draw(paint: VisualPaint, canvas: BitmapCanvas, d: Float, fonts: StyleFonts) {
        val fill = Paint(Paint.ANTI_ALIAS_FLAG).apply { style = Paint.Style.FILL }
        val stroke = Paint(Paint.ANTI_ALIAS_FLAG).apply {
            style = Paint.Style.STROKE
            strokeJoin = Paint.Join.ROUND
            strokeCap = Paint.Cap.ROUND
        }
        val rect = RectF()
        for (shape in paint.shapes) {
            when (shape) {
                is VisualShape.Rect -> {
                    rect.set(shape.x * d, shape.y * d, (shape.x + shape.w) * d, (shape.y + shape.h) * d)
                    val r = min(shape.radius * d, min(rect.width(), rect.height()) / 2)
                    fill.color = color(shape.color)
                    if (r > 0f) canvas.drawRoundRect(rect, r, r, fill) else canvas.drawRect(rect, fill)
                }
                is VisualShape.Path -> {
                    val pts = shape.points
                    if (pts.size < 4) continue
                    val path = android.graphics.Path()
                    path.moveTo(pts[0] * d, pts[1] * d)
                    var i = 2
                    while (i + 1 < pts.size) {
                        path.lineTo(pts[i] * d, pts[i + 1] * d)
                        i += 2
                    }
                    if (shape.closed) path.close()
                    shape.fill?.let {
                        fill.color = color(it)
                        canvas.drawPath(path, fill)
                    }
                    shape.stroke?.let {
                        stroke.color = color(it)
                        stroke.strokeWidth = shape.lineWidth * d
                        stroke.pathEffect = shape.dash?.let { dash -> DashPathEffect(floatArrayOf(dash * d, dash * d), 0f) }
                        canvas.drawPath(path, stroke)
                    }
                }
            }
        }
        val text = paint.text
        for (run in paint.runs) {
            val start = run.start.toInt()
            val end = start + run.len.toInt()
            if (end > text.length) continue
            val p = fonts[run.style] ?: continue
            p.color = color(run.color)
            canvas.drawText(text, start, end, run.x * d, run.baseline * d, p)
        }
    }
}
