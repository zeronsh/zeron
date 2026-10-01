package sh.zeron.android.ui

import kotlin.math.abs
import kotlin.math.exp
import kotlin.math.sin
import kotlin.math.tanh

/**
 * The effort slider's look and motion as pure functions of the level (0 = lowest
 * stop, 1 = highest), so the mapping can be unit-tested and the draw code stays
 * allocation-free: colors are packed ARGB ints, sparkles are computed from an
 * index and a clock rather than stored.
 */
object EffortFx {
    // ---- Rubber band ------------------------------------------------------

    /** The furthest the thumb may stretch past an end of the track, in dp. */
    const val MAX_STRETCH_DP = 14f

    /**
     * Rubber-band resistance: [over] is how far the finger went past the end
     * (signed, any unit), the result how far the thumb follows (same sign and
     * unit, never beyond [max]). It starts at about 0.6:1 and tightens.
     */
    fun damp(over: Float, max: Float): Float {
        if (over == 0f || max <= 0f) return 0f
        val soft = max * tanh(abs(over) / (max * RESISTANCE))
        return if (over < 0f) -soft else soft
    }

    private const val RESISTANCE = 1.6f

    /**
     * Signed overshoot of a pointer past the stops, in px: positive beyond the
     * last stop, negative beyond the first, zero between them. [raw] is the
     * unclamped rail fraction of the pointer, [travel] the px between the stops.
     */
    fun overshoot(raw: Float, travel: Float): Float = when {
        raw > 1f -> (raw - 1f) * travel
        raw < 0f -> raw * travel
        else -> 0f
    }

    // ---- Level -> look ----------------------------------------------------

    // Fill gradient stops along the level: cool blue, fresh teal, warm amber, hot red. Going straight
    // from blue to amber would pass through a muddy olive, hence the teal in between.
    private val STOPS = floatArrayOf(0f, 0.3f, 0.6f, 1f)
    private val STARTS = intArrayOf(0xFF3FA7D6.toInt(), 0xFF2FB89A.toInt(), 0xFFF0A030.toInt(), 0xFFFF3B5C.toInt())
    private val ENDS = intArrayOf(0xFF6FD6E8.toInt(), 0xFF6EDDA6.toInt(), 0xFFFF7E45.toInt(), 0xFFFFC447.toInt())

    /** Fill gradient start color (left edge) for [level]. */
    fun startColor(level: Float): Int = ramp(level, STARTS)

    /** Fill gradient end color (at the thumb) for [level]. */
    fun endColor(level: Float): Int = ramp(level, ENDS)

    private fun ramp(level: Float, colors: IntArray): Int {
        val l = level.coerceIn(0f, 1f)
        var i = 1
        while (i < STOPS.size - 1 && l > STOPS[i]) i++
        return lerpColor(colors[i - 1], colors[i], (l - STOPS[i - 1]) / (STOPS[i] - STOPS[i - 1]))
    }

    fun lerpColor(a: Int, b: Int, t: Float): Int {
        val f = t.coerceIn(0f, 1f)
        fun ch(shift: Int): Int {
            val x = (a ushr shift) and 0xFF
            val y = (b ushr shift) and 0xFF
            return (x + (y - x) * f + 0.5f).toInt().coerceIn(0, 255)
        }
        return (ch(24) shl 24) or (ch(16) shl 16) or (ch(8) shl 8) or ch(0)
    }

    /** Shimmer sweeps per second: a slow drift when low, quick when high. */
    fun shimmerSpeed(level: Float): Float {
        val l = level.coerceIn(0f, 1f)
        return 0.10f + 0.20f * l + 1.0f * l * l * l
    }

    /** Peak alpha of the white shimmer band. */
    fun shimmerAlpha(level: Float): Float {
        val l = level.coerceIn(0f, 1f)
        return 0.06f + 0.40f * l * l
    }

    /** How many sparkles are alive of the [MAX_SPARKLES] pool: none until the upper half. */
    fun sparkleCount(level: Float): Int {
        val l = level.coerceIn(0f, 1f)
        if (l < 0.5f) return 0
        return 1 + ((l - 0.5f) / 0.5f * (MAX_SPARKLES - 1) + 0.5f).toInt()
    }

    const val MAX_SPARKLES = 12

    /** Sparkle twinkles per second: lively at the top. */
    fun sparkleRate(level: Float): Float = 0.6f + 1.4f * level.coerceIn(0f, 1f)

    /** Soft glow pulse strength under the bar; only the very top pulses. */
    fun glow(level: Float): Float {
        val l = level.coerceIn(0f, 1f)
        return if (l < 0.6f) 0f else ((l - 0.6f) / 0.4f).let { it * it }
    }

    /** Glow pulses per second. */
    const val GLOW_HZ = 0.85f

    /** 0..1 brightness of the glow at [time] seconds. */
    fun glowPulse(time: Float): Float = 0.5f + 0.5f * sin(time * GLOW_HZ * TWO_PI)

    private const val TWO_PI = 6.2831855f

    // ---- Sparkles (no storage: everything derives from the index and clock)

    /** Hash an int to 0..1, stable. */
    fun hash01(n: Int): Float {
        var h = n * -0x61c88647
        h = h xor (h ushr 15)
        h *= 0x2c1b3c6d
        h = h xor (h ushr 12)
        h *= 0x297a2d39
        h = h xor (h ushr 15)
        return (h ushr 8) / 16777216f
    }

    /**
     * Life of sparkle [i] at [clock] (already scaled by the rate): 0 when dark,
     * peaks at 1 mid-life. Each slot re-rolls its position every cycle.
     */
    fun sparkleLife(i: Int, clock: Float): Float {
        val t = clock + hash01(i * 7 + 1) * 3f
        val f = t - kotlin.math.floor(t)
        return sin(f * Math.PI.toFloat()).let { it * it }
    }

    /** Which cycle sparkle [i] is in at [clock] (changes its position). */
    fun sparkleCycle(i: Int, clock: Float): Int = kotlin.math.floor(clock + hash01(i * 7 + 1) * 3f).toInt()

    fun sparkleX(i: Int, cycle: Int): Float = hash01(i * 131 + cycle * 17 + 3)

    fun sparkleY(i: Int, cycle: Int): Float = 0.18f + 0.64f * hash01(i * 97 + cycle * 29 + 5)

    fun sparkleSize(i: Int, cycle: Int): Float = 0.6f + 0.4f * hash01(i * 53 + cycle * 11 + 7)

    // ---- Arrival bursts ---------------------------------------------------

    /** Length of the power burst / zip streaks, ms. */
    const val BURST_MS = 800
    const val ZIP_MS = 600

    /** Easing of a 0..1 progress that decays fast: for fading rings. */
    fun fadeOut(t: Float): Float = (1f - t.coerceIn(0f, 1f)).let { it * it }

    /** Zip streak [i] (of [ZIP_STREAKS]) progress along the bar at [t] 0..1; staggered starts. */
    fun zipProgress(i: Int, t: Float): Float {
        val delay = hash01(i * 19 + 2) * 0.35f
        return ((t - delay) / (1f - 0.35f)).coerceIn(0f, 1f)
    }

    const val ZIP_STREAKS = 7

    /** Resting streak alpha envelope: in fast, out slower. */
    fun zipAlpha(p: Float): Float = if (p <= 0f || p >= 1f) 0f else minOf(p / 0.15f, 1f) * (1f - p)

    /** Which of the two ends a step is settled at, if either. */
    fun isMax(level: Float) = level >= 0.999f
}

/** Which end of the ladder an arrival effect belongs to. */
enum class EffortEnd { Low, High }

/**
 * Decides when an end of the ladder counts as *arrived at*: only once per visit
 * and only after the user moved there. Dragging through an end and out again
 * doesn't count (the caller applies a dwell before asking again).
 */
class ArrivalTracker {
    private var fired: EffortEnd? = null

    /** The end of [step] among [count] stops, or null in the middle (or with a single stop). */
    fun endOf(step: Int, count: Int): EffortEnd? = when {
        count < 2 -> null
        step <= 0 -> EffortEnd.Low
        step >= count - 1 -> EffortEnd.High
        else -> null
    }

    /**
     * Whether an arrival should play now. True the first time a settled step
     * is at an end ([moved] by the user); leaving the end re-arms it.
     */
    fun settle(step: Int, count: Int, moved: Boolean): EffortEnd? {
        val end = endOf(step, count)
        if (end == null) {
            fired = null
            return null
        }
        if (!moved || fired == end) return null
        fired = end
        return end
    }

    /** Remember an end as already visited (opening the picker on it plays nothing). */
    fun seed(step: Int, count: Int) {
        fired = endOf(step, count)
    }

    companion object {
        /** Dwell before a still-dragging finger counts as having arrived, ms. */
        const val DWELL_MS = 180L
    }
}

/**
 * One lightning strike as pure functions: its brightness over its life, its seed and when a strike happens at all.
 * Fast mode strikes ONCE when it is switched on; nothing is drawn or animated afterwards.
 */
object LightningFx {
    /**
     * Brightness 0..1 at [age] (0 at the first frame, 1 when the strike is over): a hard first flash, two weaker
     * restrikes (the flicker) and a bloom that fades out to nothing. Zero outside 0 until 1, continuous into it.
     */
    fun intensity(age: Float): Float {
        if (age < 0f || age >= 1f) return 0f
        val main = pulse(age, 0f, 0.09f)
        val second = 0.75f * pulse(age, 0.14f, 0.10f)
        val third = 0.5f * pulse(age, 0.30f, 0.16f)
        val rest = 1f - age
        // The afterglow: a soft bloom that lingers under the flicker and dies away smoothly.
        val glow = AFTERGLOW * rest * rest
        return (maxOf(main, second, third, glow) * rest).coerceIn(0f, 1f)
    }

    private const val AFTERGLOW = 0.45f

    private fun pulse(age: Float, at: Float, width: Float): Float {
        if (age < at) return 0f
        val d = age - at
        // The first flash is instant; restrikes ramp in over a frame or two.
        val rise = if (at == 0f) 1f else (d / 0.015f).coerceIn(0f, 1f).let { it * it * (3f - 2f * it) }
        return exp(-d / width) * rise
    }

    /** How long a strike lasts from its first frame to the last, s: flash, flicker and the bloom fading out. */
    const val LIFE_SECONDS = 0.9f

    /** With reduced motion: how long the single still frame (at the flash's peak) stays up, ms. */
    const val STILL_MILLIS = 450L

    /** The strike's age, 0..1, [elapsedNanos] after its first frame. */
    fun ageAt(elapsedNanos: Long): Float = (elapsedNanos / 1e9f / LIFE_SECONDS).coerceIn(0f, 1f)

    /**
     * Whether a change of fast mode from [was] to [now] strikes: only on switching it ON. Opening the picker with
     * fast already on ([first] composition) shows nothing, and switching off strikes nothing either.
     */
    fun strikes(first: Boolean, was: Boolean, now: Boolean): Boolean = !first && !was && now

    /** A strike's seed from its running number and some [entropy]: well mixed, so consecutive strikes look different. */
    fun seedFor(strike: Int, entropy: Long): Long {
        var z = entropy + strike * -0x61c8864680b583ebL
        z = (z xor (z ushr 30)) * -0x40a7b892e31b1a47L
        z = (z xor (z ushr 27)) * -0x6b2fb644ecceee15L
        return z xor (z ushr 31)
    }
}
