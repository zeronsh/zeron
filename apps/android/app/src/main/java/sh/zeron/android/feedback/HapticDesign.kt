package sh.zeron.android.feedback

import android.os.Build
import android.os.VibrationEffect
import android.os.VibrationEffect.Composition as C
import android.view.HapticFeedbackConstants as H

/** One primitive in a composition: [scale] 0..1 is the Standard strength, [delayMs] precedes it. */
data class Step(val primitive: Int, val scale: Float, val delayMs: Int = 0)

/** One segment of the waveform fallback: vibrate at [amplitude] (0 = pause) for [ms]. */
data class Segment(val ms: Long, val amplitude: Int)

/**
 * How a [Haptic] is felt. The engine walks a ladder from the richest effect
 * the device can render down to a plain pulse:
 *
 * 1. The platform's own `performHapticFeedback` constant (OEM tuned, so it
 *    feels native) when [preferView] and the strength is Standard.
 * 2. A `VibrationEffect.Composition` of primitives, scaled by strength.
 * 3. A view constant for patterns that are not [preferView] (Standard only).
 * 4. The designed [waveform] (amplitude control), then the predefined effect,
 *    then the waveform without amplitudes.
 */
data class HapticSpec(
    val haptic: Haptic,
    val preferView: Boolean,
    /** SDK level -> `HapticFeedbackConstants` value, or null when this device has none for the moment. */
    val view: (Int) -> Int?,
    /** Alternatives tried in order; the first whose primitives are all supported wins. */
    val compositions: List<List<Step>>,
    val predefined: Int,
    val waveform: List<Segment>,
    /** Prefer [waveform] over [predefined] when amplitude control exists (the compound patterns). */
    val designedWaveform: Boolean = false,
    /** 0 light (scrolling-class), 1 action, 2 event, 3 alert. */
    val priority: Int,
    /** The same haptic never repeats inside this many ms. */
    val minGapMs: Long,
)

object HapticTable {
    private fun api(level: Int, constant: Int): (Int) -> Int? = { sdk -> if (sdk >= level) constant else null }
    private fun api(level: Int, constant: Int, below: Int): (Int) -> Int? = { sdk -> if (sdk >= level) constant else below }

    /** Haptics whose feel depends on `haptic(h, level)`; every other one ignores the level. */
    val leveled: Set<Haptic> = setOf(Haptic.EffortStep, Haptic.Stretch)

    /** [level] 0..1 only matters to [leveled] haptics (see [effortStep], [stretch]). */
    fun spec(haptic: Haptic, level: Float = 0.5f): HapticSpec = when (haptic) {
        Haptic.EffortStep -> effortStep(level)
        Haptic.Stretch -> stretch(level)
        else -> fixed[haptic.ordinal]
    }

    private val fixed: List<HapticSpec> by lazy { Haptic.entries.map(::build) }

    private fun lerp(a: Float, b: Float, t: Float) = a + (b - a) * t

    /**
     * A thinking-power detent that grows with [level] (0 = the lowest power, 1 = the highest): from a firm,
     * crisp click to a heavy thunk, always clearly stronger than [Haptic.Select] and [Haptic.Tick] (TICK 0.7).
     * Platform view constants have a fixed, lighter strength, so this never takes that rung of the ladder.
     *
     *  - primitives: CLICK (0.70 -> 1.00) with, from level 0.2 on, a THUD under it (0.25 -> 1.00) 10 ms later,
     *    and at the top a second CLICK as the thunk lands;
     *  - without THUD: two CLICKs, the second as strong as the level (a doubled click reads heavy);
     *  - without CLICK-class strength: two full TICKs 14 ms apart;
     *  - waveform: a hard 10 ms hit, a gap, then a body 10-38 ms long, amplitudes 150-255 growing with the level.
     */
    fun effortStep(level: Float): HapticSpec {
        val l = level.coerceIn(0f, 1f)
        val click = lerp(0.70f, 1f, l)
        val thud = lerp(0.25f, 1f, ((l - 0.2f) / 0.8f).coerceIn(0f, 1f))
        val withThud = buildList {
            add(Step(C.PRIMITIVE_CLICK, click))
            if (l >= 0.2f) add(Step(C.PRIMITIVE_THUD, thud, 10))
            if (l >= 0.75f) add(Step(C.PRIMITIVE_CLICK, 1f, 22))
        }
        val clicks = buildList {
            add(Step(C.PRIMITIVE_CLICK, click))
            add(Step(C.PRIMITIVE_CLICK, lerp(0.45f, 1f, l), 16))
        }
        val ticks = listOf(Step(C.PRIMITIVE_TICK, 1f), Step(C.PRIMITIVE_TICK, lerp(0.8f, 1f, l), 14))
        return HapticSpec(
            Haptic.EffortStep, false, { null },
            listOf(withThud, clicks, ticks),
            if (l < 0.4f) VibrationEffect.EFFECT_CLICK else VibrationEffect.EFFECT_HEAVY_CLICK,
            listOf(
                Segment(10, (150 + 105 * l).toInt()),
                Segment(8, 0),
                Segment((10 + 28 * l).toLong(), (110 + 145 * l).toInt()),
            ),
            designedWaveform = true, priority = 1, minGapMs = 60,
        )
    }

    /**
     * The effort thumb pulled past the end of its track: a light tick that gets firmer the further it is pulled
     * ([level] 0 = just past the end, 1 = as far as it goes), with a low drag under it from 0.6 on.
     */
    fun stretch(level: Float): HapticSpec {
        val l = level.coerceIn(0f, 1f)
        val tick = lerp(0.25f, 0.70f, l)
        val rising = buildList {
            add(Step(C.PRIMITIVE_TICK, tick))
            if (l >= 0.6f) add(Step(C.PRIMITIVE_LOW_TICK, lerp(0.35f, 0.8f, (l - 0.6f) / 0.4f), 12))
        }
        return HapticSpec(
            Haptic.Stretch, false, { null },
            listOf(rising, listOf(Step(C.PRIMITIVE_TICK, tick)), listOf(Step(C.PRIMITIVE_LOW_TICK, lerp(0.3f, 0.9f, l)))),
            VibrationEffect.EFFECT_TICK,
            listOf(Segment(8, (40 + 110 * l).toInt())),
            priority = 0, minGapMs = 45,
        )
    }

    /** The rising ticks of [Haptic.Surge]: scale and gap per step, ending in the crack. */
    internal val surgeRamp: List<Pair<Float, Int>> = listOf(
        0.20f to 0, 0.30f to 60, 0.42f to 50, 0.56f to 40, 0.70f to 32,
    )

    private fun surge(finish: Step, closer: Step?): List<Step> = buildList {
        for ((scale, gap) in surgeRamp) add(Step(C.PRIMITIVE_TICK, scale, gap))
        add(finish)
        if (closer != null) add(closer)
    }

    private fun build(haptic: Haptic): HapticSpec = when (haptic) {
        // The faintest detent: slider steps and discrete pickers. API 34 has a dedicated frequent tick.
        Haptic.Tick -> HapticSpec(
            haptic, true, api(34, H.SEGMENT_FREQUENT_TICK, H.CLOCK_TICK),
            listOf(listOf(Step(C.PRIMITIVE_LOW_TICK, 0.5f)), listOf(Step(C.PRIMITIVE_TICK, 0.3f))),
            VibrationEffect.EFFECT_TICK, listOf(Segment(10, 40)), priority = 0, minGapMs = 70,
        )
        // A choice was made: crisper than Tick.
        Haptic.Select -> HapticSpec(
            haptic, true, api(34, H.SEGMENT_TICK, H.CONTEXT_CLICK),
            listOf(listOf(Step(C.PRIMITIVE_TICK, 0.7f)), listOf(Step(C.PRIMITIVE_CLICK, 0.35f))),
            VibrationEffect.EFFECT_TICK, listOf(Segment(12, 90)), priority = 0, minGapMs = 50,
        )
        // Switches: a rising pair for on, a falling pair for off (the system toggle constants from API 34).
        Haptic.ToggleOn -> HapticSpec(
            haptic, true, api(34, H.TOGGLE_ON),
            listOf(
                listOf(Step(C.PRIMITIVE_LOW_TICK, 0.5f), Step(C.PRIMITIVE_TICK, 0.8f, 20)),
                listOf(Step(C.PRIMITIVE_TICK, 0.5f), Step(C.PRIMITIVE_CLICK, 0.6f, 25)),
            ),
            VibrationEffect.EFFECT_CLICK, listOf(Segment(8, 60), Segment(20, 0), Segment(12, 130)), priority = 1, minGapMs = 80,
        )
        Haptic.ToggleOff -> HapticSpec(
            haptic, true, api(34, H.TOGGLE_OFF),
            listOf(
                listOf(Step(C.PRIMITIVE_TICK, 0.7f), Step(C.PRIMITIVE_LOW_TICK, 0.4f, 20)),
                listOf(Step(C.PRIMITIVE_CLICK, 0.5f), Step(C.PRIMITIVE_TICK, 0.4f, 25)),
            ),
            VibrationEffect.EFFECT_TICK, listOf(Segment(12, 110), Segment(20, 0), Segment(8, 50)), priority = 1, minGapMs = 80,
        )
        // A press that begins something.
        Haptic.Press -> HapticSpec(
            haptic, true, api(30, H.GESTURE_START),
            listOf(listOf(Step(C.PRIMITIVE_CLICK, 0.4f)), listOf(Step(C.PRIMITIVE_TICK, 0.8f))),
            VibrationEffect.EFFECT_TICK, listOf(Segment(14, 100)), priority = 1, minGapMs = 120,
        )
        // A committed action: send, create, save, start.
        Haptic.Confirm -> HapticSpec(
            haptic, true, api(30, H.CONFIRM),
            listOf(listOf(Step(C.PRIMITIVE_CLICK, 0.65f)), listOf(Step(C.PRIMITIVE_TICK, 0.9f))),
            VibrationEffect.EFFECT_CLICK, listOf(Segment(20, 170)), priority = 1, minGapMs = 120,
        )
        // Soft rise, then a small settle: a turn completed, a transfer arrived.
        Haptic.Success -> HapticSpec(
            haptic, false, { null },
            listOf(
                listOf(Step(C.PRIMITIVE_QUICK_RISE, 0.45f), Step(C.PRIMITIVE_LOW_TICK, 0.55f, 40)),
                listOf(Step(C.PRIMITIVE_CLICK, 0.5f), Step(C.PRIMITIVE_TICK, 0.5f, 60)),
            ),
            VibrationEffect.EFFECT_CLICK, listOf(Segment(24, 70), Segment(24, 130), Segment(30, 0), Segment(18, 110)),
            designedWaveform = true, priority = 2, minGapMs = 400,
        )
        // Two crisp taps: something needs you.
        Haptic.Attention -> HapticSpec(
            haptic, false, { null },
            listOf(
                listOf(Step(C.PRIMITIVE_TICK, 0.85f), Step(C.PRIMITIVE_TICK, 0.85f, 90)),
                listOf(Step(C.PRIMITIVE_CLICK, 0.6f), Step(C.PRIMITIVE_CLICK, 0.6f, 90)),
            ),
            VibrationEffect.EFFECT_DOUBLE_CLICK, listOf(Segment(18, 200), Segment(70, 0), Segment(18, 200)),
            designedWaveform = true, priority = 2, minGapMs = 400,
        )
        // A short, heavy double: failed or refused.
        Haptic.Error -> HapticSpec(
            haptic, false, api(30, H.REJECT),
            listOf(
                listOf(Step(C.PRIMITIVE_CLICK, 1f), Step(C.PRIMITIVE_THUD, 0.8f, 60)),
                listOf(Step(C.PRIMITIVE_CLICK, 1f), Step(C.PRIMITIVE_CLICK, 0.9f, 70)),
            ),
            VibrationEffect.EFFECT_HEAVY_CLICK, listOf(Segment(35, 255), Segment(55, 0), Segment(45, 230)),
            designedWaveform = true, priority = 3, minGapMs = 400,
        )
        Haptic.LongPress -> HapticSpec(
            haptic, true, api(0, H.LONG_PRESS),
            listOf(listOf(Step(C.PRIMITIVE_CLICK, 0.7f)), listOf(Step(C.PRIMITIVE_TICK, 1f))),
            VibrationEffect.EFFECT_CLICK, listOf(Segment(25, 160)), priority = 1, minGapMs = 300,
        )
        // A swipe / drag crossed the point of commitment.
        Haptic.Threshold -> HapticSpec(
            haptic, true, api(34, H.GESTURE_THRESHOLD_ACTIVATE),
            listOf(listOf(Step(C.PRIMITIVE_CLICK, 0.55f)), listOf(Step(C.PRIMITIVE_TICK, 0.9f))),
            VibrationEffect.EFFECT_CLICK, listOf(Segment(16, 150)), priority = 1, minGapMs = 150,
        )
        // Delete, uninstall, discard: a low thud with a click on top.
        Haptic.Heavy -> HapticSpec(
            haptic, false, { null },
            listOf(
                listOf(Step(C.PRIMITIVE_THUD, 0.85f), Step(C.PRIMITIVE_CLICK, 0.5f, 30)),
                listOf(Step(C.PRIMITIVE_CLICK, 1f)),
            ),
            VibrationEffect.EFFECT_HEAVY_CLICK, listOf(Segment(40, 255)), priority = 2, minGapMs = 300,
        )
        // Starred, pinned: a small pop that decays.
        Haptic.Pop -> HapticSpec(
            haptic, false, { null },
            listOf(
                listOf(Step(C.PRIMITIVE_TICK, 0.8f), Step(C.PRIMITIVE_LOW_TICK, 0.35f, 25)),
                listOf(Step(C.PRIMITIVE_TICK, 0.8f)),
            ),
            VibrationEffect.EFFECT_TICK, listOf(Segment(12, 120), Segment(14, 0), Segment(8, 50)), priority = 1, minGapMs = 120,
        )

        // Round 2. EffortStep and Stretch depend on the level and are built by effortStep / stretch.
        Haptic.EffortStep -> effortStep(0.5f)
        Haptic.Stretch -> stretch(0.5f)

        // The thumb snaps back: one firm thump that decays at once (a quiet low tick trails it).
        Haptic.Rebound -> HapticSpec(
            haptic, false, { null },
            listOf(
                listOf(Step(C.PRIMITIVE_THUD, 0.9f), Step(C.PRIMITIVE_LOW_TICK, 0.35f, 30)),
                listOf(Step(C.PRIMITIVE_CLICK, 1f), Step(C.PRIMITIVE_LOW_TICK, 0.35f, 30)),
                listOf(Step(C.PRIMITIVE_CLICK, 1f)),
            ),
            VibrationEffect.EFFECT_HEAVY_CLICK,
            listOf(Segment(22, 230), Segment(12, 100), Segment(10, 40)),
            designedWaveform = true, priority = 1, minGapMs = 150,
        )

        // The highest power: five ticks that firm up and bunch together, then a hard crack with a thud under it,
        // about 250 ms in all. Without THUD the crack is a double click; without TICK it is clicks only.
        Haptic.Surge -> HapticSpec(
            haptic, false, { null },
            listOf(
                surge(Step(C.PRIMITIVE_CLICK, 0.9f, 24), Step(C.PRIMITIVE_THUD, 1f, 8)),
                surge(Step(C.PRIMITIVE_CLICK, 0.85f, 24), Step(C.PRIMITIVE_CLICK, 1f, 14)),
                listOf(
                    Step(C.PRIMITIVE_CLICK, 0.25f), Step(C.PRIMITIVE_CLICK, 0.35f, 60), Step(C.PRIMITIVE_CLICK, 0.5f, 50),
                    Step(C.PRIMITIVE_CLICK, 0.7f, 40), Step(C.PRIMITIVE_CLICK, 1f, 30),
                ),
            ),
            VibrationEffect.EFFECT_HEAVY_CLICK,
            listOf(
                Segment(28, 40), Segment(18, 0), Segment(28, 70), Segment(14, 0), Segment(28, 110), Segment(12, 0),
                Segment(28, 160), Segment(10, 0), Segment(28, 210), Segment(8, 0), Segment(40, 255),
            ),
            designedWaveform = true, priority = 2, minGapMs = 500,
        )

        // The lowest power: a very short, sharp double tick.
        Haptic.Zip -> HapticSpec(
            haptic, false, { null },
            listOf(
                listOf(Step(C.PRIMITIVE_TICK, 0.9f), Step(C.PRIMITIVE_TICK, 0.9f, 28)),
                listOf(Step(C.PRIMITIVE_CLICK, 0.5f), Step(C.PRIMITIVE_CLICK, 0.5f, 28)),
            ),
            VibrationEffect.EFFECT_TICK,
            listOf(Segment(6, 200), Segment(18, 0), Segment(6, 200)),
            designedWaveform = true, priority = 1, minGapMs = 100,
        )

        // Fast mode on: an irregular crackle of micro-pulses (uneven strength and gaps) ending in a firm crack,
        // about 180 ms.
        Haptic.Lightning -> HapticSpec(
            haptic, false, { null },
            listOf(
                listOf(
                    Step(C.PRIMITIVE_TICK, 0.45f), Step(C.PRIMITIVE_TICK, 0.80f, 31), Step(C.PRIMITIVE_LOW_TICK, 0.35f, 17),
                    Step(C.PRIMITIVE_TICK, 0.90f, 44), Step(C.PRIMITIVE_CLICK, 1f, 52),
                ),
                listOf(
                    Step(C.PRIMITIVE_CLICK, 0.40f), Step(C.PRIMITIVE_CLICK, 0.70f, 36), Step(C.PRIMITIVE_CLICK, 0.35f, 20),
                    Step(C.PRIMITIVE_CLICK, 0.85f, 44), Step(C.PRIMITIVE_CLICK, 1f, 52),
                ),
            ),
            VibrationEffect.EFFECT_DOUBLE_CLICK,
            listOf(
                Segment(7, 110), Segment(22, 0), Segment(5, 220), Segment(13, 0), Segment(6, 70), Segment(34, 0),
                Segment(8, 255), Segment(40, 0), Segment(24, 255),
            ),
            designedWaveform = true, priority = 2, minGapMs = 400,
        )

        // Jumping to a provider on the rail: lighter than a slider Tick, the faintest detent there is.
        Haptic.RailTick -> HapticSpec(
            haptic, true, api(34, H.SEGMENT_FREQUENT_TICK, H.CLOCK_TICK),
            listOf(listOf(Step(C.PRIMITIVE_LOW_TICK, 0.3f)), listOf(Step(C.PRIMITIVE_TICK, 0.2f))),
            VibrationEffect.EFFECT_TICK, listOf(Segment(6, 28)), priority = 0, minGapMs = 45,
        )
    }

    val all: List<HapticSpec> get() = fixed
}

/** What the device can do, probed once at start. */
data class HapticCapabilities(
    val sdk: Int,
    /** Primitive ids this vibrator renders (`areAllPrimitivesSupported`). */
    val primitives: Set<Int>,
    val amplitudeControl: Boolean,
    /** A window view is attached for `performHapticFeedback`. */
    val hasView: Boolean,
)

/** The effect chosen for one haptic on one device. */
sealed interface HapticPlan {
    data class ViewConstant(val constant: Int) : HapticPlan
    data class Composition(val steps: List<Step>) : HapticPlan
    data class Predefined(val effect: Int) : HapticPlan
    data class Waveform(val timings: LongArray, val amplitudes: IntArray?) : HapticPlan {
        override fun equals(other: Any?) = other is Waveform && timings.contentEquals(other.timings) && amplitudes.contentEquals(other.amplitudes)
        override fun hashCode() = timings.contentHashCode() * 31 + amplitudes.contentHashCode()
    }

    /** Nothing to play, with why (kept for the debug log). */
    data class Skip(val reason: String) : HapticPlan
}

object HapticPlanner {
    /** Primitive scale at [strength], never inaudible and never above full. */
    fun scaled(scale: Float, strength: HapticStrength): Float = (scale * strength.scale).coerceIn(0.05f, 1f)

    fun plan(haptic: Haptic, strength: HapticStrength, caps: HapticCapabilities, level: Float = 0.5f): HapticPlan {
        val spec = HapticTable.spec(haptic, level)
        val standard = strength == HapticStrength.Standard
        val view = if (caps.hasView && standard) spec.view(caps.sdk) else null

        if (spec.preferView && view != null) return HapticPlan.ViewConstant(view)

        if (caps.sdk >= Build.VERSION_CODES.R) {
            spec.compositions.firstOrNull { steps -> steps.all { it.primitive in caps.primitives } }?.let { steps ->
                return HapticPlan.Composition(steps.map { it.copy(scale = scaled(it.scale, strength)) })
            }
        }
        if (view != null) return HapticPlan.ViewConstant(view)

        // Hardware without primitives. Without amplitude control a Subtle request can only be honoured
        // by dropping the lightest haptics: playing them at full strength would be the opposite.
        if (!caps.amplitudeControl && strength == HapticStrength.Subtle && spec.priority == 0) {
            return HapticPlan.Skip("subtle: no amplitude control")
        }
        if (caps.amplitudeControl && (spec.designedWaveform || !standard)) return waveform(spec, strength, true)
        if (standard || !caps.amplitudeControl) return HapticPlan.Predefined(spec.predefined)
        return waveform(spec, strength, caps.amplitudeControl)
    }

    fun waveform(spec: HapticSpec, strength: HapticStrength, amplitudes: Boolean): HapticPlan.Waveform {
        if (!amplitudes) return HapticPlan.Waveform(onOffTimings(spec.waveform), null)
        val timings = LongArray(spec.waveform.size) { spec.waveform[it].ms }
        val amps = IntArray(spec.waveform.size) {
            val a = spec.waveform[it].amplitude
            if (a == 0) 0 else (a * strength.scale).toInt().coerceIn(1, 255)
        }
        return HapticPlan.Waveform(timings, amps)
    }

    /** `createWaveform(timings, repeat)` alternates off, on, off, ... starting with off. */
    fun onOffTimings(segments: List<Segment>): LongArray {
        val out = ArrayList<Long>()
        var lastOn = false // the implicit leading slot is "off"
        out.add(0L)
        for (segment in segments) {
            val isOn = segment.amplitude > 0
            if (isOn == lastOn) out[out.lastIndex] = out.last() + segment.ms else out.add(segment.ms)
            lastOn = isOn
        }
        return out.toLongArray()
    }
}
