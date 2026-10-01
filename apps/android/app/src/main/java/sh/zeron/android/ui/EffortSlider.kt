package sh.zeron.android.ui

import android.provider.Settings
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.animate
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.focusable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.State
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawWithCache
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.RoundRect
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.clipPath
import androidx.compose.ui.graphics.drawscope.translate
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.input.pointer.PointerId
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.positionChanged
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.ProgressBarRangeInfo
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.progressBarRangeInfo
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.setProgress
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import sh.zeron.android.design.LocalDarkTheme
import sh.zeron.android.feedback.Cue
import sh.zeron.android.feedback.Haptic
import sh.zeron.android.feedback.LocalFeedback
import kotlin.math.PI
import kotlin.math.abs
import kotlin.math.cos
import kotlin.math.floor
import kotlin.math.roundToInt
import kotlin.math.sin

/** One stop on the slider: the wire id and the name shown. */
data class EffortLevel(val id: String, val label: String)

private val SliderHeight = 52.dp
private val RailHeight = 34.dp
private val ThumbWidth = 56.dp
private val ThumbWidthPressed = 66.dp
private val ThumbHeight = 44.dp
private val ThumbHeightPressed = 46.dp

/** The user turned animations off system-wide (Developer options / Remove animations). */
@Composable
fun rememberReduceMotion(): Boolean {
    val resolver = LocalContext.current.contentResolver
    return remember { Settings.Global.getFloat(resolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f }
}

/**
 * Developer options' "Animator duration scale" for the thumb's own spring (Android's animators honour it, a hand-rolled
 * spring must do so itself): 1 normally, 10 plays everything ten times slower, which is how motion is inspected frame by
 * frame. Zero (animations off) is [rememberReduceMotion], not a speed.
 */
@Composable
internal fun rememberAnimatorScale(): Float {
    val resolver = LocalContext.current.contentResolver
    return remember { Settings.Global.getFloat(resolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f).coerceIn(0.1f, 100f) }
}

/** A looping 0…1 phase, only running while composed (callers gate it on there being something to animate). */
@Composable
internal fun rememberPhase(periodMillis: Int): State<Float> {
    val transition = rememberInfiniteTransition(label = "phase")
    return transition.animateFloat(0f, 1f, infiniteRepeatable(tween(periodMillis, easing = LinearEasing)), label = "phase")
}

/**
 * Seconds since the slider appeared, and a shimmer phase that integrates the
 * level's shimmer speed (so a faster level speeds the sweep up without the band
 * jumping). Both are plain state read only by draw blocks, so a frame costs a
 * redraw of the small canvases and no recomposition.
 */
@Stable
private class FxClock {
    var time by mutableFloatStateOf(0f)
    var shimmer by mutableFloatStateOf(0f)
}

@Composable
private fun rememberFxClock(running: Boolean, level: State<Float>): FxClock {
    val clock = remember { FxClock() }
    LaunchedEffect(running) {
        if (!running) return@LaunchedEffect
        var last = 0L
        while (true) {
            androidx.compose.runtime.withFrameNanos { now ->
                val dt = if (last == 0L) 0f else ((now - last) / 1e9f).coerceAtMost(0.1f)
                last = now
                clock.time = (clock.time + dt) % 600f
                clock.shimmer = (clock.shimmer + dt * EffortFx.shimmerSpeed(level.value)) % 1000f
            }
        }
    }
    return clock
}

/**
 * The reasoning-effort slider: a pill rail with a dot at every level and a
 * springy pill thumb you drag, fling or tap, snapping to the levels. Every
 * level crossed fires one [Haptic.EffortStep] that is firmer the higher the
 * level, with a rising cue.
 *
 * The bar's colour and life follow the level: a calm cool fill and slow drift
 * at the bottom, warmer in the middle, and a hot gradient with a quick shimmer,
 * sparkles and a pulsing glow at the top. Dragging past either end stretches
 * the thumb with rubber-band resistance ([Haptic.Stretch], once) and on release
 * it bounces back ([Haptic.Rebound]). Settling on the top level plays a power
 * burst ([Haptic.Surge], [Cue.Surge]); settling on the lowest plays quick
 * streaks ([Haptic.Zip], [Cue.Zip]). [fast] adds speed streaks to the fill.
 *
 * Accessible as a slider: its state reads "High, 3 of 5", TalkBack's set
 * progress and two custom actions step it, and arrow keys / Home / End
 * work on hardware keyboards.
 */
@Composable
fun EffortSlider(
    levels: List<EffortLevel>,
    selected: Int,
    onSelected: (Int) -> Unit,
    modifier: Modifier = Modifier,
    fast: Boolean = false,
    description: String = "Reasoning effort",
) {
    val count = levels.size
    if (count == 0) return
    val step = selected.coerceIn(0, count - 1)
    val feedback = LocalFeedback.current
    val reduceMotion = rememberReduceMotion()
    val animatorScale = rememberAnimatorScale()
    val currentOnSelected by rememberUpdatedState(onSelected)
    val currentFeedback by rememberUpdatedState(feedback)
    val scope = rememberCoroutineScope()

    var dragging by remember { mutableStateOf(false) }
    var railWidth by remember { mutableFloatStateOf(0f) }
    val tracker = remember(count) { DetentTracker(count, step) }
    // A step set from outside (reset, accessibility, a new model) retargets the tracker.
    LaunchedEffect(step, count) { tracker.jump(step) }

    // The ONE motion state: the thumb's centre in steps (0 = first level; below 0 / above count - 1 while stretched
    // past an end). Thumb, fill, dots and halo are all derived from it, so they can never disagree.
    val spring = remember(count) { ThumbSpring(step.toFloat()) }
    var thumbX by remember(count) { mutableFloatStateOf(step.toFloat()) }
    // The finger, in steps (unbounded): while it is down the thumb is exactly under it.
    val finger = remember(count) { floatArrayOf(step.toFloat()) }
    // Mode of the thumb: under a dragging finger, dropping into a level, or rebounding from a stretch.
    var engaged by remember { mutableStateOf(false) }
    var rebounding by remember { mutableStateOf(false) }
    // The level the thumb settles on when not dragging (set at release before the parent's recomposition catches up).
    var restStep by remember(count) { mutableIntStateOf(step) }
    val loopRunning = remember { booleanArrayOf(false) }
    // [entered the stretch, reached the wall] of the current pull, for the Stretch haptics.
    val pull = remember { booleanArrayOf(false, false) }
    val density = LocalDensity.current
    // Arrival bursts: 0 when idle, 0..1 while playing.
    var arrival by remember { mutableFloatStateOf(0f) }
    var arrivalEnd by remember { mutableStateOf(EffortEnd.High) }
    var arrivalJob by remember { mutableStateOf<Job?>(null) }
    val arrivals = remember(count) { ArrivalTracker().also { it.seed(step, count) } }
    var userMoved by remember { mutableStateOf(false) }
    var rebounded by remember { mutableStateOf(false) }

    /** One detent: report the step and feel it, firmer the higher it is. */
    fun detent(next: Int) {
        userMoved = true
        currentOnSelected(next)
        val f = currentFeedback
        f.haptic(Haptic.EffortStep, EffortScale.fraction(next, count))
        f.cue(Cue.Detent, next)
    }

    fun arrive(end: EffortEnd) {
        val f = currentFeedback
        if (end == EffortEnd.High) f.both(Haptic.Surge, Cue.Surge) else f.both(Haptic.Zip, Cue.Zip)
        if (reduceMotion) return
        arrivalJob?.cancel()
        arrivalEnd = end
        arrivalJob = scope.launch {
            animate(0f, 1f, animationSpec = tween(if (end == EffortEnd.High) EffortFx.BURST_MS else EffortFx.ZIP_MS, easing = LinearEasing)) { v, _ -> arrival = v }
            arrival = 0f
        }
    }

    // Arriving at an end plays once per visit: after a short dwell while the finger is still down, soon after release.
    LaunchedEffect(step, dragging, count) {
        if (arrivals.endOf(step, count) == null) {
            arrivals.settle(step, count, moved = false)
            userMoved = false
            return@LaunchedEffect
        }
        if (!userMoved) return@LaunchedEffect
        delay(if (dragging) ArrivalTracker.DWELL_MS else if (rebounded) 150L else 60L)
        arrivals.settle(step, count, moved = true)?.let { arrive(it) }
        rebounded = false
        userMoved = false
    }

    // One frame of the thumb's life; true once it is at rest and nothing is dragging (the loop then stops: idle costs nothing).
    fun tick(dt: Float): Boolean {
        val span = (count - 1).coerceAtLeast(1)
        val inset = with(density) { ThumbWidth.toPx() } / 2
        val travel = (railWidth - 2 * inset).coerceAtLeast(1f)
        val stepPx = travel / span
        val maxStretch = with(density) { EffortFx.MAX_STRETCH_DP.dp.toPx() } / stepPx
        val busy = dragging && engaged
        val target: Float
        if (busy) {
            target = EffortDrag.follow(finger[0], count, maxStretch)
            // Stretch haptics fire once per pull: entering it, then reaching the wall.
            val mag = EffortGeometry.stretch(target, count, travel)
            if (!pull[0] && mag > 1.5f * density.density) {
                pull[0] = true
                currentFeedback.haptic(Haptic.Stretch, 0.35f)
            } else if (pull[0] && mag < 0.5f * density.density) {
                pull[0] = false
                pull[1] = false
            }
            if (pull[0] && !pull[1] && mag > 0.8f * maxStretch * stepPx) {
                pull[1] = true
                currentFeedback.haptic(Haptic.Stretch, 1f)
            }
        } else {
            target = restStep.toFloat()
        }
        if (reduceMotion || busy) {
            // Under the finger: no spring, no lag.
            spring.snapTo(target)
        } else {
            val (k, z) = when {
                rebounding -> EffortTuning.REBOUND_STIFFNESS to EffortTuning.REBOUND_DAMPING_RATIO
                else -> EffortTuning.SETTLE_STIFFNESS to EffortTuning.SETTLE_DAMPING_RATIO
            }
            spring.step(target, dt / animatorScale, k, z)
        }
        // The spring may swing a little past a level, but never further than the rubber band allows.
        spring.x = spring.x.coerceIn(-maxStretch, (count - 1) + maxStretch)
        thumbX = spring.x
        // The selection changes when the thumb itself crosses over, so the haptic lands with the spring.
        if (busy) tracker.update((spring.x / span).coerceIn(0f, 1f))?.let { restStep = it; detent(it) }
        if (!busy && spring.isSettled(target)) {
            spring.snapTo(target)
            thumbX = target
            rebounding = false
            return true
        }
        return false
    }

    fun ensureLoop() {
        if (loopRunning[0]) return
        loopRunning[0] = true
        scope.launch {
            var last = 0L
            while (true) {
                var done = false
                androidx.compose.runtime.withFrameNanos { now ->
                    val dt = if (last == 0L) 1f / 60f else ((now - last) / 1e9f).coerceIn(0.001f, 0.05f)
                    last = now
                    done = tick(dt)
                    // Cleared in the frame itself so a gesture starting right now can restart the loop.
                    if (done) loopRunning[0] = false
                }
                if (done) break
            }
        }
    }

    // A level set from outside (reset, accessibility, a new model) or by a tap: the thumb springs to it.
    LaunchedEffect(step, count) {
        restStep = step
        if (!dragging) ensureLoop()
    }

    val thumbWidth by animateDpAsState(if (dragging) ThumbWidthPressed else ThumbWidth, MaterialTheme.motionScheme.fastSpatialSpec(), label = "thumbWidth")
    val thumbHeight by animateDpAsState(if (dragging) ThumbHeightPressed else ThumbHeight, MaterialTheme.motionScheme.fastSpatialSpec(), label = "thumbHeight")
    val lift by animateDpAsState(if (dragging) 8.dp else 3.dp, MaterialTheme.motionScheme.fastEffectsSpec(), label = "lift")
    val level = animateFloatAsState(EffortScale.fraction(step, count), tween(380), label = "level")
    val power by animateFloatAsState(if (fast) 1f else 0f, tween(420), label = "power")
    val clock = rememberFxClock(running = !reduceMotion, level = level)

    val scheme = MaterialTheme.colorScheme
    val dark = LocalDarkTheme.current
    val trackColor = scheme.onSurface.copy(alpha = if (dark) 0.10f else 0.075f)
    val outline = scheme.onSurface.copy(alpha = if (dark) 0.10f else 0.08f)
    val thumbColor = if (dark) Color(0xFFF3F3F6) else Color.White

    Box(
        modifier
            .fillMaxWidth()
            .height(SliderHeight)
            .onSizeChanged { railWidth = it.width.toFloat() }
            .semantics(mergeDescendants = true) {
                contentDescription = description
                stateDescription = "${levels[step].label}, ${step + 1} of $count"
                progressBarRangeInfo = ProgressBarRangeInfo(step.toFloat(), 0f..(count - 1).toFloat(), steps = (count - 2).coerceAtLeast(0))
                setProgress { value ->
                    val to = value.roundToInt().coerceIn(0, count - 1)
                    if (to != step) detent(to)
                    true
                }
                customActions = listOf(
                    CustomAccessibilityAction("Increase effort") { if (step < count - 1) detent(step + 1); step < count - 1 },
                    CustomAccessibilityAction("Decrease effort") { if (step > 0) detent(step - 1); step > 0 },
                )
            }
            .focusable()
            .onKeyEvent { event ->
                if (event.type != KeyEventType.KeyDown) return@onKeyEvent false
                val to = when (event.key) {
                    Key.DirectionRight, Key.DirectionUp, Key.PageUp -> (step + 1).coerceAtMost(count - 1)
                    Key.DirectionLeft, Key.DirectionDown, Key.PageDown -> (step - 1).coerceAtLeast(0)
                    Key.MoveHome -> 0
                    Key.MoveEnd -> count - 1
                    else -> return@onKeyEvent false
                }
                if (to != step) detent(to)
                true
            }
            .pointerInput(count) {
                val inset = ThumbWidth.toPx() / 2
                awaitEachGesture {
                    val down = awaitFirstDown(requireUnconsumed = false)
                    down.consume()
                    val width = size.width.toFloat()
                    val travel = (width - 2 * inset).coerceAtLeast(1f)
                    val span = (count - 1).coerceAtLeast(1)
                    fun stepsAt(x: Float) = EffortScale.rawFractionAt(x, width, inset) * span
                    var moved = false
                    engaged = false
                    rebounding = false
                    pull[0] = false
                    pull[1] = false
                    finger[0] = stepsAt(down.position.x)
                    dragging = true
                    try {
                        var id: PointerId = down.id
                        while (true) {
                            val event = awaitPointerEvent()
                            val change = event.changes.firstOrNull { it.id == id } ?: break
                            if (change.positionChanged()) {
                                finger[0] = stepsAt(change.position.x)
                                if (!engaged && abs(change.position.x - down.position.x) > viewConfiguration.touchSlop) {
                                    // Out of the tap: from here the thumb is under the finger.
                                    moved = true
                                    engaged = true
                                    ensureLoop()
                                }
                                change.consume()
                            }
                            if (!change.pressed) break
                        }
                        // A tap lands on the stop under the finger; a drag on the stop nearest to where it let go. No fling.
                        val landAt = if (engaged) finger[0] / span else EffortScale.fractionAt(down.position.x, width, inset)
                        val land = EffortScale.nearestStep(landAt.coerceIn(0f, 1f), count)
                        if (land != tracker.step) {
                            tracker.jump(land)
                            detent(land)
                        }
                        restStep = land
                        if (EffortGeometry.stretch(spring.x, count, travel) > EffortTuning.STRETCH_REBOUND_MIN_DP.dp.toPx()) {
                            // Let go while stretched: the thumb springs back with a bounce.
                            rebounded = true
                            rebounding = true
                            currentFeedback.haptic(Haptic.Rebound)
                            currentFeedback.cue(Cue.Rebound)
                        } else if (moved && EffortScale.isEnd(land, count).not()) {
                            currentFeedback.haptic(Haptic.Select)
                        }
                    } finally {
                        engaged = false
                        dragging = false
                        ensureLoop()
                    }
                }
            },
    ) {
        // Under the rail: the pulsing glow of the very top level.
        Canvas(Modifier.fillMaxSize()) {
            val glow = EffortFx.glow(level.value)
            if (glow <= 0.001f) return@Canvas
            val pulse = if (reduceMotion) 0.7f else 0.35f + 0.65f * EffortFx.glowPulse(clock.time)
            val k = glow * pulse
            val color = Color(EffortFx.endColor(level.value))
            val railTop = (size.height - RailHeight.toPx()) / 2
            val corner = CornerRadius(RailHeight.toPx() / 2)
            val rail = Size(size.width, RailHeight.toPx())
            drawRoundRect(color.copy(alpha = 0.10f * k), Offset(-8.dp.toPx(), railTop - 8.dp.toPx()), Size(rail.width + 16.dp.toPx(), rail.height + 16.dp.toPx()), CornerRadius(corner.x + 8.dp.toPx()), style = Stroke(10.dp.toPx()))
            drawRoundRect(color.copy(alpha = 0.18f * k), Offset(-3.dp.toPx(), railTop - 3.dp.toPx()), Size(rail.width + 6.dp.toPx(), rail.height + 6.dp.toPx()), CornerRadius(corner.x + 3.dp.toPx()), style = Stroke(5.dp.toPx()))
        }
        Box(
            Modifier
                .align(Alignment.Center)
                .fillMaxWidth()
                .height(RailHeight)
                .clip(CircleShape)
                .background(trackColor)
                .border(1.dp, outline, CircleShape),
        ) {
            // The fill: a capsule from the rail's left edge whose right cap is tucked under the thumb. Its end is derived
            // from the same thumb position as the thumb itself (EffortGeometry), so the two can never disagree, and it stays
            // round while the thumb stretches past an end or springs back.
            Canvas(Modifier.fillMaxSize()) {
                val inset = ThumbWidth.toPx() / 2
                val travel = size.width - 2 * inset
                val fillRight = fillRightPx(thumbX, count, inset, travel, size.height)
                drawRoundRect(
                    Brush.horizontalGradient(
                        listOf(Color(EffortFx.startColor(level.value)), Color(EffortFx.endColor(level.value))),
                        startX = 0f,
                        endX = fillRight.coerceAtLeast(1f),
                    ),
                    size = Size(fillRight, size.height),
                    cornerRadius = CornerRadius(size.height / 2),
                )
                for (i in 0 until count) {
                    val x = inset + EffortScale.fraction(i, count) * travel
                    val on = x < fillRight - 2.dp.toPx()
                    drawCircle(
                        if (on) Color.White.copy(alpha = 0.7f) else scheme.onSurface.copy(alpha = 0.30f),
                        radius = 2.2.dp.toPx(),
                        center = Offset(x, size.height / 2),
                    )
                }
            }
            // The fill's life: shimmer, sparkles and (fast mode) streaks.
            Box(
                Modifier
                    .fillMaxSize()
                    .drawWithCache {
                        val band = (size.width * 0.34f).coerceAtLeast(60.dp.toPx())
                        val sweep = Brush.horizontalGradient(listOf(Color.Transparent, Color.White, Color.Transparent), startX = 0f, endX = band)
                        val streak = Path()
                        val capsule = Path()
                        onDrawBehind {
                            val inset = ThumbWidth.toPx() / 2
                            val fillEnd = fillRightPx(thumbX, count, inset, size.width - 2 * inset, size.height)
                            if (fillEnd <= 0f) return@onDrawBehind
                            val lv = level.value
                            val phase = if (reduceMotion) 0.5f else clock.shimmer - floor(clock.shimmer)
                            // Clipped to the fill's own capsule so the shimmer never shows a square end.
                            capsule.rewind()
                            capsule.addRoundRect(RoundRect(0f, 0f, fillEnd, size.height, CornerRadius(size.height / 2)))
                            clipPath(capsule) {
                                // Shimmer band sweeping across the filled part.
                                val x = -band + (fillEnd + band) * phase
                                translate(left = x) {
                                    drawRect(sweep, size = Size(band, size.height), alpha = EffortFx.shimmerAlpha(lv) + 0.08f * power)
                                }
                                if (power > 0.001f) drawStreaks(streak, phase, power, fillEnd)
                                if (!reduceMotion) drawSparkles(lv, clock.time, fillEnd)
                            }
                        }
                    },
            )
        }
        // Over the rail, under the thumb: fast mode's halo and the arrival bursts.
        Canvas(Modifier.fillMaxSize()) {
            val inset = ThumbWidth.toPx() / 2
            val travel = size.width - 2 * inset
            val centre = EffortGeometry.centre(thumbX, count, inset, travel)
            val cy = size.height / 2
            val lv = level.value
            if (power > 0.001f) {
                val breath = 0.55f + 0.45f * (0.5f + 0.5f * sin((if (reduceMotion) 0.25f else clock.time * 0.45f) * 2f * PI.toFloat()))
                // Stacked discs rather than a gradient: nothing is allocated per frame.
                val halo = Color(EffortFx.endColor(lv))
                drawCircle(halo.copy(alpha = 0.10f * power * breath), radius = 38.dp.toPx(), center = Offset(centre, cy))
                drawCircle(halo.copy(alpha = 0.16f * power * breath), radius = 28.dp.toPx(), center = Offset(centre, cy))
                drawCircle(halo.copy(alpha = 0.22f * power * breath), radius = 20.dp.toPx(), center = Offset(centre, cy))
            }
            val t = arrival
            if (t > 0f && t < 1f) {
                if (arrivalEnd == EffortEnd.High) drawSurge(t, size.width, cy, inset) else drawZip(t, size.width, cy, inset)
            }
        }
        Box(
            Modifier
                .offset {
                    val inset = ThumbWidth.toPx() / 2
                    IntOffset((EffortGeometry.centre(thumbX, count, inset, railWidth - 2 * inset) - thumbWidth.toPx() / 2).roundToInt(), 0)
                }
                .align(Alignment.CenterStart)
                .size(thumbWidth, thumbHeight)
                .graphicsLayer {
                    // Stretched past an end: the pill widens about its centre (the fill's end is tucked under its centre, so
                    // it stays covered) and flattens a little. Zero between the stops, and while the spring swings back inside.
                    val inset = ThumbWidth.toPx() / 2
                    val pull = EffortGeometry.stretch(thumbX, count, railWidth - 2 * inset)
                    val w = thumbWidth.toPx()
                    scaleX = 1f + 2f * EffortTuning.THUMB_STRETCH_GAIN * pull / w
                    scaleY = 1f - 0.08f * (pull / EffortFx.MAX_STRETCH_DP.dp.toPx()).coerceIn(0f, 1f)
                }
                .shadow(lift, CircleShape, ambientColor = Color.Black.copy(alpha = 0.5f), spotColor = Color.Black.copy(alpha = 0.5f))
                .background(thumbColor, CircleShape),
        )
    }
}

/** The fill's right end in px for a thumb at [x] steps (see [EffortGeometry.fillRight]), on a rail [railHeight] tall. */
private fun fillRightPx(x: Float, count: Int, inset: Float, travel: Float, railHeight: Float): Float {
    val centre = EffortGeometry.centre(x, count, inset, travel)
    val half = EffortGeometry.thumbHalf(inset, EffortGeometry.stretch(x, count, travel))
    return EffortGeometry.fillRight(centre, half, railHeight)
}

/** Slanted speed streaks racing toward the thumb (fast mode). */
private fun DrawScope.drawStreaks(streak: Path, phase: Float, power: Float, fillEnd: Float) {
    val gap = 34.dp.toPx()
    val slant = size.height * 0.6f
    val width = 7.dp.toPx()
    val speed = ((phase * 3f) % 1f) * gap
    var sx = -gap + speed
    while (sx < fillEnd + gap) {
        streak.reset()
        streak.moveTo(sx, size.height)
        streak.lineTo(sx + width, size.height)
        streak.lineTo(sx + width + slant, 0f)
        streak.lineTo(sx + slant, 0f)
        streak.close()
        drawPath(streak, Color.White, alpha = 0.16f * power)
        sx += gap
    }
}

/** Twinkling glints scattered over the fill, more and livelier the higher the level. */
private fun DrawScope.drawSparkles(level: Float, time: Float, fillEnd: Float) {
    val n = EffortFx.sparkleCount(level)
    if (n == 0) return
    val rate = EffortFx.sparkleRate(level)
    val unit = 1.dp.toPx()
    for (i in 0 until n) {
        val life = EffortFx.sparkleLife(i, time * rate)
        if (life < 0.03f) continue
        val cycle = EffortFx.sparkleCycle(i, time * rate)
        val x = EffortFx.sparkleX(i, cycle) * fillEnd
        val y = EffortFx.sparkleY(i, cycle) * size.height
        val r = (1.4f + 1.4f * EffortFx.sparkleSize(i, cycle)) * unit * life
        val centre = Offset(x, y)
        drawCircle(Color.White.copy(alpha = 0.9f * life), radius = r * 0.55f, center = centre)
        val arm = r * 2.4f
        val c = Color.White.copy(alpha = 0.75f * life)
        drawLine(c, Offset(x - arm, y), Offset(x + arm, y), strokeWidth = unit * 0.9f, cap = StrokeCap.Round)
        drawLine(c, Offset(x, y - arm), Offset(x, y + arm), strokeWidth = unit * 0.9f, cap = StrokeCap.Round)
    }
}

/** The top level's power burst: a flare sweeping the bar, two energy rings and rays from the thumb. */
private fun DrawScope.drawSurge(t: Float, width: Float, cy: Float, inset: Float) {
    val unit = 1.dp.toPx()
    // Strong colours: the burst plays over a light or dark card, not just over the bar's own fill.
    val hot = Color(EffortFx.startColor(1f))
    val gold = Color(0xFFFFB020)
    val core = Color(0xFFFFF1C7)
    val cx = width - inset
    // A bright flare racing from the left end to the thumb.
    val sweepT = (t / 0.55f).coerceIn(0f, 1f)
    val sweepX = (width - 2 * inset) * (1f - (1f - sweepT) * (1f - sweepT)) + inset
    val flareAlpha = if (t < 0.55f) 1f else EffortFx.fadeOut((t - 0.55f) / 0.45f)
    val bandH = RailHeight.toPx()
    drawRoundRect(core.copy(alpha = 0.35f * flareAlpha), Offset(sweepX - 26 * unit, cy - bandH / 2), Size(52 * unit, bandH), CornerRadius(bandH / 2))
    drawRoundRect(Color.White.copy(alpha = 0.8f * flareAlpha), Offset(sweepX - 5 * unit, cy - bandH / 2), Size(10 * unit, bandH), CornerRadius(5 * unit))
    // Energy rings from the thumb.
    val ring1 = t
    drawCircle(hot.copy(alpha = 0.9f * EffortFx.fadeOut(ring1)), radius = (8 + 74 * ring1) * unit, center = Offset(cx, cy), style = Stroke(((9f * (1f - ring1)) + 1.5f) * unit))
    if (t > 0.15f) {
        val ring2 = (t - 0.15f) / 0.85f
        drawCircle(gold.copy(alpha = 0.85f * EffortFx.fadeOut(ring2)), radius = (6 + 52 * ring2) * unit, center = Offset(cx, cy), style = Stroke(((6f * (1f - ring2)) + 1f) * unit))
    }
    // Core flash.
    val flash = EffortFx.fadeOut(t * 1.4f)
    drawCircle(core.copy(alpha = 0.28f * flash), radius = 40 * unit, center = Offset(cx, cy))
    drawCircle(Color.White.copy(alpha = 0.45f * flash), radius = 20 * unit, center = Offset(cx, cy))
    // Rays fanning out the open side.
    val rayAlpha = EffortFx.fadeOut(t)
    for (k in 0..6) {
        val a = (PI / 2 + k * PI / 6).toFloat()
        val r0 = (16 + 40 * t) * unit
        val r1 = r0 + (10 + 12 * (1f - t)) * unit
        drawLine(
            gold.copy(alpha = 0.95f * rayAlpha),
            Offset(cx + cos(a) * r0, cy + sin(a) * r0),
            Offset(cx + cos(a) * r1, cy + sin(a) * r1),
            strokeWidth = 2.5f * unit,
            cap = StrokeCap.Round,
        )
    }
}

/** The lowest level's "faster and lighter": cool streaks racing right to left along the bar, then a pale wipe. */
private fun DrawScope.drawZip(t: Float, width: Float, cy: Float, inset: Float) {
    val unit = 1.dp.toPx()
    val left = inset - 14 * unit
    val right = width - 6 * unit
    val h = RailHeight.toPx()
    // Deep cyan on the outside so the streaks read on a pale track, white-hot at the head.
    val cool = Color(0xFF2BB6E6)
    val ice = Color(0xFF3FA7D6)
    for (i in 0 until EffortFx.ZIP_STREAKS) {
        val p = EffortFx.zipProgress(i, t)
        val a = EffortFx.zipAlpha(p)
        if (a <= 0.01f) continue
        val lane = cy - h / 2 + h * (i + 0.5f) / EffortFx.ZIP_STREAKS
        val head = right - (right - left) * p
        val len = (34 + 38 * EffortFx.hash01(i * 5 + 1)) * unit
        // Brightest at the head, trailing off behind it.
        drawLine(ice.copy(alpha = 0.55f * a), Offset(head, lane), Offset(head + len, lane), strokeWidth = 3f * unit, cap = StrokeCap.Round)
        drawLine(cool.copy(alpha = 0.9f * a), Offset(head, lane), Offset(head + len * 0.55f, lane), strokeWidth = 2f * unit, cap = StrokeCap.Round)
        drawLine(Color.White.copy(alpha = 0.95f * a), Offset(head, lane), Offset(head + len * 0.22f, lane), strokeWidth = 1.2f * unit, cap = StrokeCap.Round)
    }
    // A pale wipe following the streaks.
    val wipe = ((t - 0.1f) / 0.7f).coerceIn(0f, 1f)
    if (wipe in 0.01f..0.99f) {
        val x = right - (right - left) * wipe
        drawRoundRect(cool.copy(alpha = 0.30f * (1f - wipe)), Offset(x - 20 * unit, cy - h / 2), Size(40 * unit, h), CornerRadius(h / 2))
    }
}
