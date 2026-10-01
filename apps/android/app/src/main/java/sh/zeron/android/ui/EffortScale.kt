package sh.zeron.android.ui

import kotlin.math.abs
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt
import kotlin.math.sqrt

/**
 * The effort slider's pure geometry and detent logic, kept apart from the
 * composable so it can be unit-tested: how a finger position becomes a step,
 * when a step *change* counts (hysteresis), where a fling lands and which
 * levels shimmer.
 */
object EffortScale {
    /**
     * Extra distance, in steps, a position must travel past the midpoint
     * between two stops before the slider moves to the next one. Jitter around
     * a boundary therefore doesn't re-fire a detent: coming back needs the same
     * margin on the other side.
     */
    const val HYSTERESIS = 0.12f

    /** How far ahead, in seconds, a release velocity is projected when picking the landing step. */
    const val FLING_HORIZON = 0.12f

    /** A step's place along the rail, 0 (first) … 1 (last). One step sits at 0. */
    fun fraction(step: Int, count: Int): Float =
        if (count <= 1) 0f else step.coerceIn(0, count - 1).toFloat() / (count - 1)

    /** The step whose stop is closest to [fraction]. */
    fun nearestStep(fraction: Float, count: Int): Int =
        if (count <= 1) 0 else (fraction.coerceIn(0f, 1f) * (count - 1)).roundToInt()

    /**
     * Position → step with hysteresis: [current] holds while the position stays
     * within half a step plus [HYSTERESIS] of its stop; past that, the nearest
     * stop wins.
     */
    fun snap(fraction: Float, count: Int, current: Int, hysteresis: Float = HYSTERESIS): Int {
        if (count <= 1) return 0
        val held = current.coerceIn(0, count - 1)
        val position = fraction.coerceIn(0f, 1f) * (count - 1)
        return if (abs(position - held) <= 0.5f + hysteresis) held else nearestStep(fraction, count)
    }

    /**
     * Pointer x → rail fraction. The thumb's centre travels between two insets
     * (half a thumb) so it never leaves the rail; the stops sit on that path.
     */
    fun fractionAt(x: Float, width: Float, inset: Float): Float {
        val travel = width - 2 * inset
        return if (travel <= 0f) 0f else ((x - inset) / travel).coerceIn(0f, 1f)
    }

    /** The pointer's unclamped rail fraction: below 0 or above 1 when the finger is past the first or last stop. */
    fun rawFractionAt(x: Float, width: Float, inset: Float): Float {
        val travel = width - 2 * inset
        return if (travel <= 0f) 0f else (x - inset) / travel
    }

    /**
     * Where a release at [fraction] moving [velocity] (rail fractions per second) would settle with a
     * fling. The slider no longer flings (a flick must not skip levels, see [EffortTuning]); kept for
     * callers that want a projected landing.
     */
    fun landing(fraction: Float, velocity: Float, count: Int, current: Int): Int =
        snap(fraction + velocity * FLING_HORIZON, count, current)

    /** Whether [step] is one of the rail's two ends (they get a firmer tick). */
    fun isEnd(step: Int, count: Int): Boolean = count > 1 && (step == 0 || step == count - 1)

    /**
     * How much a level shimmers, 0…1: only the top of the catalog's ladder
     * (`xhigh`, `max`, then the `ultra*` levels) glows, more the higher it is.
     */
    fun energy(level: String): Float = when (level.lowercase()) {
        "xhigh" -> 0.35f
        "max" -> 0.65f
        "ultra", "ultracode", "ultrathink" -> 1f
        else -> 0f
    }
}

/**
 * Tracks the step under a moving pointer and reports only *changes*: one
 * detent per crossing, never per frame, and not again on jitter at a boundary
 * (see [EffortScale.snap]).
 */
class DetentTracker(private val count: Int, start: Int) {
    var step: Int = start.coerceIn(0, (count - 1).coerceAtLeast(0))
        private set

    /** The new step when [fraction] moved onto another one, else null. */
    fun update(fraction: Float): Int? {
        val next = EffortScale.snap(fraction, count, step)
        if (next == step) return null
        step = next
        return next
    }

    /** Adopt a step set from outside (a keyboard, an accessibility action, a reset). */
    fun jump(to: Int) {
        step = to.coerceIn(0, (count - 1).coerceAtLeast(0))
    }
}


/**
 * Every constant that sets how the effort selector *feels*, in one place.
 * Positions in the drag model are in "steps": 0 is the first level, `count - 1`
 * the last, and one unit is the distance between two levels.
 */
object EffortTuning {
    /** Spring of the thumb dropping into a level after release or a tap: slight overshoot, springs into the detent. */
    const val SETTLE_STIFFNESS = 430f
    const val SETTLE_DAMPING_RATIO = 0.6f

    /** Spring of the thumb snapping back from a rubber-band stretch: loose and bouncy. */
    const val REBOUND_STIFFNESS = 420f
    const val REBOUND_DAMPING_RATIO = 0.3f

    /** The thumb grows by this many px (per side) for each px it is stretched past an end. */
    const val THUMB_STRETCH_GAIN = 0.3f

    /** A pull past an end is only counted as stretched above this many dp. */
    const val STRETCH_REBOUND_MIN_DP = 1f
}

/**
 * The finger-to-thumb mapping of the effort selector, pure so it can be tested. While a finger is down the
 * thumb is exactly under it (no stickiness, no lag, no inertia); only past an end does it rubber-band.
 * On release the thumb springs to the nearest level ([EffortTuning.SETTLE_STIFFNESS]).
 */
object EffortDrag {
    /**
     * The thumb for a finger at [p] (steps, unbounded): [p] itself between the first and last level, and past
     * an end the rubber band, [EffortFx.damp] of how far the finger got, never more than [maxStretch] steps
     * past the end. Monotonic and continuous everywhere.
     */
    fun follow(p: Float, count: Int, maxStretch: Float): Float {
        if (count <= 1) return 0f
        val last = (count - 1).toFloat()
        return when {
            p > last -> last + EffortFx.damp(p - last, maxStretch)
            p < 0f -> -EffortFx.damp(-p, maxStretch)
            else -> p
        }
    }
}

/** A spring-damper chasing a target, integrated in small fixed sub-steps so it stays stable at any frame time. */
class ThumbSpring(var x: Float = 0f) {
    var v: Float = 0f

    fun step(target: Float, dt: Float, stiffness: Float, dampingRatio: Float) {
        val damping = 2f * dampingRatio * sqrt(stiffness)
        var left = dt.coerceIn(0f, 0.1f)
        while (left > 1e-6f) {
            val h = min(left, SUBSTEP)
            // Semi-implicit Euler: velocity first, then position with the new velocity.
            v += (-stiffness * (x - target) - damping * v) * h
            x += v * h
            left -= h
        }
    }

    fun isSettled(target: Float, eps: Float = 0.002f): Boolean = abs(x - target) < eps && abs(v) < eps * 5f

    fun snapTo(target: Float) {
        x = target
        v = 0f
    }

    private companion object {
        const val SUBSTEP = 1f / 240f
    }
}

/**
 * The effort bar's drawn geometry, derived from ONE number: the thumb's centre `x` in steps (below 0 or above
 * `count - 1` while stretched past an end). Thumb and fill both read it, so they cannot disagree.
 */
object EffortGeometry {
    /** Thumb centre in px for position [x] (steps) on a rail whose stops sit between [inset] and `inset + travel`. */
    fun centre(x: Float, count: Int, inset: Float, travel: Float): Float =
        if (count <= 1) inset else inset + x / (count - 1) * travel

    /** How far past the nearest end the thumb is, px, never negative (zero between the stops). */
    fun stretch(x: Float, count: Int, travel: Float): Float {
        if (count <= 1) return 0f
        val step = travel / (count - 1)
        val last = (count - 1).toFloat()
        return when {
            x > last -> (x - last) * step
            x < 0f -> -x * step
            else -> 0f
        }
    }

    /** Half the thumb's drawn width: [baseHalf] widened by the stretch. */
    fun thumbHalf(baseHalf: Float, stretch: Float): Float = baseHalf + EffortTuning.THUMB_STRETCH_GAIN * stretch

    /**
     * Where the fill's right end sits. The fill is a capsule from the rail's left edge; its right cap is
     * tucked under the thumb: at least at the thumb's centre, at most at the thumb's far edge, nominally
     * a rail-radius past the centre. Never shorter than the rail height (it keeps a round left cap).
     */
    fun fillRight(centre: Float, thumbHalf: Float, railHeight: Float): Float {
        val tuck = min(railHeight / 2f, max(thumbHalf - FILL_MARGIN, 0f))
        return (centre + tuck).coerceAtLeast(railHeight).coerceAtMost(centre + thumbHalf)
    }

    private const val FILL_MARGIN = 2f
}
