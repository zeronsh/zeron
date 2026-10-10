package sh.zeron.android.transcript

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.tween
import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.foundation.gestures.Orientation
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.rememberScrollableState
import androidx.compose.foundation.gestures.scrollable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.interaction.DragInteraction
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.overscroll
import androidx.compose.foundation.rememberOverscrollEffect
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.ContentCopy
import androidx.compose.material.icons.outlined.FileCopy
import androidx.compose.material.icons.outlined.TextFields
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.drawscope.drawIntoCanvas
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.nativeCanvas
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.Layout
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.distinctUntilChanged
import sh.zeron.android.design.LocalDarkTheme
import sh.zeron.android.design.ZIcons
import sh.zeron.android.design.TranscriptPalette
import uniffi.zeron_core.LayoutFrame
import uniffi.zeron_core.RowPlacement
import kotlin.math.abs
import kotlin.math.exp
import kotlin.math.floor
import kotlin.math.min
import kotlin.math.roundToInt
import kotlin.math.pow

/** What the transcript asks of its host (links, images, text sheets). */
class TranscriptActions(
    val openUrl: (String) -> Unit,
    val loadImage: suspend (String) -> androidx.compose.ui.graphics.ImageBitmap?,
    val showText: (title: String, text: String, mono: Boolean) -> Unit,
)

private const val OVERSCAN = 700f
private const val BAND = 256f

/**
 * Virtualized transcript. Only rows intersecting the viewport (plus overscan)
 * are composed; each paints its Rust display list on a canvas at the exact
 * Rust coordinates, so measurement and rendering can't disagree.
 */
@Composable
fun Transcript(state: TranscriptState, actions: TranscriptActions, modifier: Modifier = Modifier) {
    val density = LocalDensity.current
    val d = density.density
    val scheme = MaterialTheme.colorScheme
    val dark = LocalDarkTheme.current
    val palette = remember(scheme, dark) { TranscriptPalette(dark, scheme) }
    val interactions = remember { MutableInteractionSource() }
    val scrollable = rememberScrollableState { px -> -state.scrollBy(-px / d) * d }
    val overscroll = rememberOverscrollEffect()

    LaunchedEffect(interactions) {
        interactions.interactions.collect { i ->
            when (i) {
                is DragInteraction.Start -> {
                    state.dragging = true
                    state.released()
                }
                is DragInteraction.Stop, is DragInteraction.Cancel -> {
                    state.dragging = false
                    if (state.distanceFromBottom < 70f) state.following = true
                }
            }
        }
    }

    // Follow spring: a critically damped approach to the tail, run only
    // while there's distance to cover (idle transcripts don't tick).
    LaunchedEffect(state) {
        snapshotFlow { state.following && !state.dragging && abs(state.maxOffset - state.offset) > 0.5f }
            .distinctUntilChanged()
            .collectLatest { chasing ->
                if (!chasing) return@collectLatest
                var last = withFrameNanos { it }
                while (true) {
                    val now = withFrameNanos { it }
                    val dt = min(1f / 30f, (now - last) / 1e9f)
                    last = now
                    val target = state.maxOffset
                    val delta = target - state.offset
                    if (abs(delta) < 0.5f) {
                        state.offset = target
                        break
                    }
                    state.offset += if (state.runwayActive) {
                        // Runway glide: the desktop's frame-rate-independent ease-out
                        // (15% of the remaining glide per 60 fps frame).
                        delta * (1f - 0.85f.pow(min(8f, dt * 60f)))
                    } else {
                        // Critically damped: the tail settles into place.
                        delta * (1f - exp(-dt * 16f))
                    }
                }
            }
    }

    BoxWithConstraints(
        modifier
            .clipToBounds()
            .overscroll(overscroll)
            .scrollable(
                scrollable,
                Orientation.Vertical,
                overscrollEffect = overscroll,
                interactionSource = interactions,
            ),
    ) {
        val widthDp = maxWidth.value
        val heightDp = maxHeight.value
        state.setViewport(widthDp, heightDp, density.fontScale)
        val frame = state.frame ?: return@BoxWithConstraints
        if (frame.styleCount().toInt() != state.fonts.count) state.fonts.update(frame.styles(), d)

        // The realized band moves in coarse steps: scrolling within a band
        // only re-places rows, it never recomposes.
        val band by remember(state) {
            derivedStateOf { floor((state.offset - OVERSCAN) / BAND) * BAND }
        }
        val motion = state.motion
        val progress = remember { Animatable(1f) }
        LaunchedEffect(motion?.id) {
            if (motion == null) return@LaunchedEffect
            progress.snapTo(0f)
            progress.animateTo(
                1f,
                tween(motion.durationMs, easing = if (motion.expo) CubicBezierEasing(0.16f, 1f, 0.3f, 1f) else FastOutSlowInEasing),
            )
        }
        val placements = remember(frame, band, heightDp) {
            frame.rowsIn(band, band + heightDp + OVERSCAN * 2 + BAND)
        }
        Layout(
            content = {
                for (p in placements) {
                    key(p.key) { RowHost(state, frame, p, palette, actions) }
                }
            },
        ) { measurables, constraints ->
            val w = constraints.maxWidth
            // Mid-motion, rows interpolate from the previous frame's placement.
            val t = progress.value
            val from = if (t < 1f) motion?.from else null
            fun lerp(a: Float, b: Float) = a + (b - a) * t
            val placeables = measurables.mapIndexed { i, m ->
                val p = placements[i]
                val h = from?.get(p.key)?.let { lerp(it.second, p.height) } ?: p.height
                m.measure(Constraints.fixed(w, (h * d).roundToInt().coerceAtLeast(0)))
            }
            layout(w, constraints.maxHeight) {
                val off = state.offset
                placeables.forEachIndexed { i, pl ->
                    val p = placements[i]
                    val y = from?.get(p.key)?.let { lerp(it.first, p.y) } ?: p.y
                    pl.placeRelative(0, ((y - off) * d).roundToInt())
                }
            }
        }
    }
}

@Composable
private fun RowHost(
    state: TranscriptState,
    frame: LayoutFrame,
    p: RowPlacement,
    palette: TranscriptPalette,
    actions: TranscriptActions,
) {
    val model = remember(p.key, p.version, frame.width()) { state.model(frame, p.index, p.key, p.version) } ?: return
    val d = LocalDensity.current.density
    val fonts = state.fonts

    // Streaming veil: each appended chunk fades in on its own clock, so a
    // new token never restarts (or pops) the ones still fading. Chunks are
    // recorded during composition — before the new text is ever drawn.
    val veil = remember(p.key) { Veil() }
    remember(model) { veil.grow(model); Unit }
    val clock = remember(p.key) { mutableLongStateOf(0L) }
    LaunchedEffect(model) {
        while (veil.active(System.nanoTime())) {
            withFrameNanos { clock.longValue = it }
        }
        clock.longValue = System.nanoTime()
    }

    // Rows that arrive after the first frame fade in.
    val appear = remember(p.key) { Animatable(if (state.knownKeys.add(p.key) && state.settled) 0f else 1f) }
    LaunchedEffect(p.key) { if (appear.value < 1f) appear.animateTo(1f, tween(280)) }

    var menuAt by remember { mutableStateOf<Offset?>(null) }
    val haptics = LocalHapticFeedback.current

    Box(
        Modifier
            .clipToBounds()
            .graphicsLayer { alpha = appear.value }
            .drawBehind {
                clock.longValue // redraw on every veil frame
                drawIntoCanvas { c -> veil.draw(c.nativeCanvas, model, d, fonts, palette, System.nanoTime()) }
            }
            .pointerTaps(model, d, actions, onLongPress = {
                haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                menuAt = it
            }),
    ) {
        model.display.scrollers.forEachIndexed { i, s ->
            val scroll = rememberScrollState()
            Box(
                Modifier
                    .offset(s.x.dp, s.y.dp)
                    .size(s.w.dp, s.h.dp)
                    .horizontalScroll(scroll),
            ) {
                Box(
                    Modifier
                        .size(s.contentWidth.dp, s.h.dp)
                        .drawBehind { drawIntoCanvas { model.draw(it.nativeCanvas, i + 1, d, fonts, palette) } },
                ) {
                    RowWidgets(state, model, scroller = i.toUInt(), palette, actions)
                }
            }
        }
        RowWidgets(state, model, scroller = null, palette, actions)
        menuAt?.let { at -> RowMenu(state, frame, p, model, at, actions) { menuAt = null } }
    }
}

/** Link taps (row coordinates) and the long-press menu. */
private fun Modifier.pointerTaps(model: RowModel, d: Float, actions: TranscriptActions, onLongPress: (Offset) -> Unit) =
    pointerInput(model) {
        detectTapGestures(
            onTap = { pos ->
                val x = pos.x / d
                val y = pos.y / d
                model.display.links.firstOrNull { l ->
                    l.scroller == null && x >= l.x - 4 && x <= l.x + l.w + 4 && y >= l.y - 2 && y <= l.y + l.h + 2
                }?.let { actions.openUrl(it.url) }
            },
            onLongPress = onLongPress,
        )
    }

@Composable
private fun RowMenu(
    state: TranscriptState,
    frame: LayoutFrame,
    p: RowPlacement,
    model: RowModel,
    at: Offset,
    actions: TranscriptActions,
    dismiss: () -> Unit,
) {
    val clipboard = LocalClipboardManager.current
    val density = LocalDensity.current
    val block = model.display.copyText
    val message = remember(frame, p.index) { frame.messageText(p.index) }
    Box(Modifier.offset(with(density) { at.x.toDp() }, with(density) { at.y.toDp() })) {
        val selectable = message ?: block
        sh.zeron.android.ui.ActionMenu(
            true,
            dismiss,
            listOfNotNull(
                if (block.isNotEmpty()) sh.zeron.android.ui.MenuAction("Copy", ZIcons.Copy) { clipboard.setText(AnnotatedString(block)) } else null,
                if (!message.isNullOrEmpty() && message != block) sh.zeron.android.ui.MenuAction("Copy message", ZIcons.Chat) { clipboard.setText(AnnotatedString(message)) } else null,
                if (selectable.isNotEmpty()) sh.zeron.android.ui.MenuAction("Select text", ZIcons.Text) { actions.showText("Select text", selectable, false) } else null,
            ),
        )
    }
}

/**
 * Fade-in bookkeeping for one streaming row: each inserted chunk of text
 * (UTF-16 `[from, to)`) with its start time. Chunks come from a prefix +
 * suffix diff, not a prefix check: list rows carry their bullet after the
 * item text, so streamed words land *before* the row's tail.
 */
private class Veil {
    private class Chunk(var from: Int, var to: Int, val started: Long)

    private var last: RowModel? = null
    private val chunks = ArrayList<Chunk>()

    fun grow(model: RowModel) {
        val prev = last
        last = model
        if (prev == null || prev === model) return
        val old = prev.display.text
        val new = model.display.text
        val delta = new.length - old.length
        if (delta <= 0) {
            chunks.clear()
            return
        }
        var p = 0
        val max = old.length
        while (p < max && old[p] == new[p]) p++
        var sfx = 0
        while (sfx < max - p && old[old.length - 1 - sfx] == new[new.length - 1 - sfx]) sfx++
        if (new.length - p - sfx != delta) {
            // More than an insertion (the block was re-flowed): show it as is.
            chunks.clear()
            return
        }
        for (c in chunks) {
            if (c.from >= p) {
                c.from += delta
                c.to += delta
            }
        }
        chunks.add(Chunk(p, p + delta, System.nanoTime()))
        chunks.sortBy { it.from }
    }

    fun active(now: Long): Boolean {
        chunks.removeAll { now - it.started >= DURATION }
        return chunks.isNotEmpty()
    }

    fun draw(canvas: android.graphics.Canvas, model: RowModel, d: Float, fonts: StyleFonts, palette: sh.zeron.android.design.TranscriptPalette, now: Long) {
        val live = chunks.filter { now - it.started < DURATION }
        if (live.isEmpty()) {
            model.draw(canvas, 0, d, fonts, palette)
            return
        }
        model.draw(canvas, 0, d, fonts, palette, RowModel.Pass.Chrome)
        // Settled text: everything between the fading chunks.
        var cursor = 0
        for (c in live) {
            if (c.from > cursor) model.draw(canvas, 0, d, fonts, palette, RowModel.Pass.Range(cursor, c.from))
            cursor = maxOf(cursor, c.to)
        }
        model.draw(canvas, 0, d, fonts, palette, RowModel.Pass.Range(cursor, Int.MAX_VALUE))
        for (c in live) {
            val t = ((now - c.started).toFloat() / DURATION).coerceIn(0f, 1f)
            val alpha = 1f - (1f - t) * (1f - t) // ease-out
            val save = canvas.saveLayerAlpha(null, (alpha * 255).toInt())
            model.draw(canvas, 0, d, fonts, palette, RowModel.Pass.Range(c.from, c.to))
            canvas.restoreToCount(save)
        }
    }

    companion object {
        const val DURATION = 220_000_000L
    }
}
