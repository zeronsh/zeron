package sh.zeron.android.feedback

import android.os.VibrationEffect.Composition as C
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test

class HapticDesignTest {
    private val allPrimitives = setOf(C.PRIMITIVE_CLICK, C.PRIMITIVE_TICK, C.PRIMITIVE_LOW_TICK, C.PRIMITIVE_THUD, C.PRIMITIVE_SPIN, C.PRIMITIVE_QUICK_RISE, C.PRIMITIVE_SLOW_RISE, C.PRIMITIVE_QUICK_FALL)

    private fun caps(sdk: Int = 36, primitives: Set<Int> = allPrimitives, amplitude: Boolean = true, view: Boolean = true) =
        HapticCapabilities(sdk, primitives, amplitude, view)

    @Test fun everyHapticHasASpec() {
        assertEquals(Haptic.entries.size, HapticTable.all.size)
        for (h in Haptic.entries) {
            val s = HapticTable.spec(h)
            assertEquals(h, s.haptic)
            assertTrue("$h has compositions", s.compositions.isNotEmpty())
            assertTrue("$h has a waveform", s.waveform.isNotEmpty() && s.waveform.any { it.amplitude > 0 })
            assertTrue("$h gap", s.minGapMs > 0)
        }
    }

    @Test fun everyHapticPlansOnEveryDevice() {
        val devices = listOf(
            caps(), caps(sdk = 29, primitives = emptySet(), view = true), caps(sdk = 29, primitives = emptySet(), amplitude = false, view = false),
            caps(sdk = 30, primitives = setOf(C.PRIMITIVE_CLICK, C.PRIMITIVE_TICK)), caps(sdk = 34, view = false),
        )
        for (h in Haptic.entries) for (strength in HapticStrength.entries) for (d in devices) {
            val plan = HapticPlanner.plan(h, strength, d)
            if (plan is HapticPlan.Skip) {
                assertTrue("only the lightest haptics may be dropped: $h $strength $d", strength == HapticStrength.Subtle && !d.amplitudeControl && HapticTable.spec(h).priority == 0)
            }
            if (plan is HapticPlan.Composition) assertTrue(plan.steps.all { it.primitive in d.primitives })
            if (plan is HapticPlan.Waveform) assertEquals(plan.timings.size, plan.amplitudes?.size ?: plan.timings.size)
        }
    }

    @Test fun standardPrefersThePlatformConstantWhereItFits() {
        assertEquals(HapticPlan.ViewConstant(android.view.HapticFeedbackConstants.SEGMENT_FREQUENT_TICK), HapticPlanner.plan(Haptic.Tick, HapticStrength.Standard, caps(sdk = 34)))
        assertEquals(HapticPlan.ViewConstant(android.view.HapticFeedbackConstants.CLOCK_TICK), HapticPlanner.plan(Haptic.Tick, HapticStrength.Standard, caps(sdk = 31)))
        assertEquals(HapticPlan.ViewConstant(android.view.HapticFeedbackConstants.TOGGLE_ON), HapticPlanner.plan(Haptic.ToggleOn, HapticStrength.Standard, caps(sdk = 34)))
        assertEquals(HapticPlan.ViewConstant(android.view.HapticFeedbackConstants.TOGGLE_OFF), HapticPlanner.plan(Haptic.ToggleOff, HapticStrength.Standard, caps(sdk = 34)))
        assertEquals(HapticPlan.ViewConstant(android.view.HapticFeedbackConstants.CONFIRM), HapticPlanner.plan(Haptic.Confirm, HapticStrength.Standard, caps(sdk = 30)))
    }

    @Test fun toggleOnAndOffDiffer() {
        // On rises (soft to crisp), off falls (crisp to soft): never the same pattern.
        for (d in listOf(caps(sdk = 31), caps(sdk = 29, primitives = emptySet()))) {
            assertFalse(HapticPlanner.plan(Haptic.ToggleOn, HapticStrength.Standard, d) == HapticPlanner.plan(Haptic.ToggleOff, HapticStrength.Standard, d))
        }
        val on = HapticTable.spec(Haptic.ToggleOn).compositions.first()
        val off = HapticTable.spec(Haptic.ToggleOff).compositions.first()
        assertTrue(on.last().scale > on.first().scale)
        assertTrue(off.last().scale < off.first().scale)
    }

    @Test fun compoundPatternsAreDesignedNotPlatformConstants() {
        for (h in listOf(Haptic.Success, Haptic.Attention, Haptic.Heavy)) {
            val plan = HapticPlanner.plan(h, HapticStrength.Standard, caps(sdk = 36))
            assertTrue("$h $plan", plan is HapticPlan.Composition && plan.steps.size >= 2)
        }
        // Error goes through the composition too; REJECT is only its fallback.
        assertTrue(HapticPlanner.plan(Haptic.Error, HapticStrength.Standard, caps()) is HapticPlan.Composition)
        assertEquals(HapticPlan.ViewConstant(android.view.HapticFeedbackConstants.REJECT), HapticPlanner.plan(Haptic.Error, HapticStrength.Standard, caps(primitives = emptySet())))
    }

    @Test fun strengthScalesPrimitives() {
        fun first(h: Haptic, s: HapticStrength) = (HapticPlanner.plan(h, s, caps(sdk = 34, view = false)) as HapticPlan.Composition).steps.first().scale
        val subtle = first(Haptic.Select, HapticStrength.Subtle)
        val standard = first(Haptic.Select, HapticStrength.Standard)
        val strong = first(Haptic.Select, HapticStrength.Strong)
        assertTrue("$subtle < $standard < $strong", subtle < standard && standard < strong)
        assertEquals(0.7f * 0.5f, subtle, 1e-6f)
        assertEquals(1f, HapticPlanner.scaled(0.9f, HapticStrength.Strong), 0f)
        assertEquals(0.05f, HapticPlanner.scaled(0.01f, HapticStrength.Subtle), 0f)
    }

    @Test fun subtleOrStrongNeverUseTheFixedStrengthPlatformConstant() {
        assertTrue(HapticPlanner.plan(Haptic.Tick, HapticStrength.Subtle, caps(sdk = 34)) is HapticPlan.Composition)
        assertTrue(HapticPlanner.plan(Haptic.Tick, HapticStrength.Strong, caps(sdk = 34)) is HapticPlan.Composition)
    }

    @Test fun waveformFallbackScalesAmplitude() {
        val none = caps(sdk = 29, primitives = emptySet(), view = false)
        val standard = HapticPlanner.plan(Haptic.Confirm, HapticStrength.Strong, none) as HapticPlan.Waveform
        val spec = HapticTable.spec(Haptic.Confirm)
        assertEquals((spec.waveform.first().amplitude * 1.5f).toInt(), standard.amplitudes!!.first())
        val subtle = HapticPlanner.plan(Haptic.Confirm, HapticStrength.Subtle, none) as HapticPlan.Waveform
        assertTrue(subtle.amplitudes!!.first() < spec.waveform.first().amplitude)
        assertTrue(standard.amplitudes!!.all { it in 0..255 })
    }

    @Test fun noAmplitudeControlUsesPredefinedAndDropsLightOnesWhenSubtle() {
        val basic = caps(sdk = 29, primitives = emptySet(), amplitude = false, view = false)
        assertTrue(HapticPlanner.plan(Haptic.Confirm, HapticStrength.Standard, basic) is HapticPlan.Predefined)
        assertTrue(HapticPlanner.plan(Haptic.Tick, HapticStrength.Subtle, basic) is HapticPlan.Skip)
        assertNotNull(HapticPlanner.plan(Haptic.Error, HapticStrength.Subtle, basic))
    }

    @Test fun onOffTimingsAlternateStartingOff() {
        val t = HapticPlanner.onOffTimings(listOf(Segment(10, 100), Segment(20, 0), Segment(5, 50)))
        assertEquals(listOf(0L, 10L, 20L, 5L), t.toList())
        val merged = HapticPlanner.onOffTimings(listOf(Segment(10, 100), Segment(5, 200)))
        assertEquals(listOf(0L, 15L), merged.toList())
    }

    @Test fun priorityOrdering() {
        assertTrue(HapticTable.spec(Haptic.Tick).priority < HapticTable.spec(Haptic.Confirm).priority)
        assertTrue(HapticTable.spec(Haptic.Confirm).priority < HapticTable.spec(Haptic.Error).priority)
    }

    // --- round 2: the real designs

    private val levels = listOf(0f, 0.25f, 0.5f, 0.75f, 1f)

    private fun steps(h: Haptic, level: Float = 0.5f, c: HapticCapabilities = caps(sdk = 36, view = false), strength: HapticStrength = HapticStrength.Standard) =
        (HapticPlanner.plan(h, strength, c, level) as HapticPlan.Composition).steps

    private fun wave(h: Haptic, level: Float = 0.5f, strength: HapticStrength = HapticStrength.Standard) =
        HapticPlanner.waveform(HapticTable.spec(h, level), strength, true)

    @Test fun designedHapticsPlanAsWaveformsWithoutPrimitives() {
        val bare = caps(sdk = 29, primitives = emptySet(), view = false)
        for (h in listOf(Haptic.EffortStep, Haptic.Rebound, Haptic.Surge, Haptic.Zip, Haptic.Lightning)) {
            assertEquals("$h", wave(h), HapticPlanner.plan(h, HapticStrength.Standard, bare))
        }
    }

    private fun duration(steps: List<Step>) = steps.sumOf { it.delayMs }

    @Test fun everyLeveledHapticPlansAtEveryLevelOnEveryDevice() {
        val devices = listOf(
            caps(), caps(sdk = 29, primitives = emptySet()), caps(sdk = 29, primitives = emptySet(), amplitude = false, view = false),
            caps(sdk = 30, primitives = setOf(C.PRIMITIVE_CLICK, C.PRIMITIVE_TICK)), caps(sdk = 31, primitives = setOf(C.PRIMITIVE_CLICK)),
        )
        for (h in Haptic.entries) for (level in levels + listOf(-3f, 7f)) for (strength in HapticStrength.entries) for (d in devices) {
            val plan = HapticPlanner.plan(h, strength, d, level)
            if (plan is HapticPlan.Composition) assertTrue("$h $level", plan.steps.all { it.primitive in d.primitives && it.scale in 0.05f..1f })
            if (plan is HapticPlan.Waveform) assertTrue("$h $level", plan.amplitudes?.all { it in 0..255 } ?: true)
        }
    }

    @Test fun effortStepGrowsFromACrispClickToAHeavyThunk() {
        var lastTotal = 0f
        var lastFirst = 0f
        for (level in levels) {
            val s = steps(Haptic.EffortStep, level)
            assertEquals(C.PRIMITIVE_CLICK, s.first().primitive)
            assertTrue("first step at $level", s.first().scale >= lastFirst)
            val total = s.sumOf { it.scale.toDouble() }.toFloat()
            assertTrue("total at $level: $total vs $lastTotal", total > lastTotal)
            lastTotal = total
            lastFirst = s.first().scale
        }
        assertTrue(steps(Haptic.EffortStep, 0f).none { it.primitive == C.PRIMITIVE_THUD })
        val top = steps(Haptic.EffortStep, 1f)
        assertTrue(top.any { it.primitive == C.PRIMITIVE_THUD && it.scale == 1f })
        assertEquals(1f, top.first().scale, 0f)
        assertTrue(top.size >= 3)
    }

    @Test fun effortStepIsStrongerThanTheOldSelectAndDetentEvenAtItsLightestLevel() {
        val old = HapticTable.spec(Haptic.Select).compositions.first().first().scale // TICK 0.7
        val tick = HapticTable.spec(Haptic.Tick).compositions.first().first().scale
        val lightest = steps(Haptic.EffortStep, 0f).first()
        assertTrue(lightest.scale >= old)
        assertTrue(lightest.scale > tick)
        // Click-class and never the faint platform constant: even Standard with a view attached stays a composition.
        assertTrue(HapticPlanner.plan(Haptic.EffortStep, HapticStrength.Standard, caps(sdk = 36, view = true), 0f) is HapticPlan.Composition)
        assertTrue(HapticPlanner.plan(Haptic.EffortStep, HapticStrength.Standard, caps(sdk = 36, view = true), 1f) is HapticPlan.Composition)
    }

    @Test fun effortStepWaveformFallbackAlsoScalesWithLevel() {
        var lastAmp = 0
        var lastMs = 0L
        for (level in levels) {
            val w = wave(Haptic.EffortStep, level)
            assertEquals(w.timings.size, w.amplitudes!!.size)
            val peak = w.amplitudes!!.max()
            assertTrue("peak at $level", peak >= lastAmp)
            assertTrue("length at $level", w.timings.sum() >= lastMs)
            lastAmp = peak
            lastMs = w.timings.sum()
        }
        assertEquals(255, wave(Haptic.EffortStep, 1f).amplitudes!!.max())
        assertTrue(wave(Haptic.EffortStep, 0f).amplitudes!!.max() >= 150) // a hard hit even at the lightest level
    }

    @Test fun effortStepFallbackLadderOnLimitedHardware() {
        // No THUD: a doubled click, the second as strong as the level.
        val noThud = caps(sdk = 31, primitives = setOf(C.PRIMITIVE_CLICK, C.PRIMITIVE_TICK), view = false)
        val clicks = steps(Haptic.EffortStep, 1f, noThud)
        assertTrue(clicks.all { it.primitive == C.PRIMITIVE_CLICK } && clicks.size == 2)
        // Only TICK: two full ticks.
        val tickOnly = caps(sdk = 31, primitives = setOf(C.PRIMITIVE_TICK), view = false)
        assertTrue(steps(Haptic.EffortStep, 0.5f, tickOnly).all { it.primitive == C.PRIMITIVE_TICK })
        // Nothing: the predefined effect steps up from click to heavy click with the level.
        val bare = caps(sdk = 29, primitives = emptySet(), amplitude = false, view = false)
        assertEquals(HapticPlan.Predefined(android.os.VibrationEffect.EFFECT_CLICK), HapticPlanner.plan(Haptic.EffortStep, HapticStrength.Standard, bare, 0.1f))
        assertEquals(HapticPlan.Predefined(android.os.VibrationEffect.EFFECT_HEAVY_CLICK), HapticPlanner.plan(Haptic.EffortStep, HapticStrength.Standard, bare, 0.9f))
    }

    @Test fun strengthStillScalesEffortStep() {
        val subtle = steps(Haptic.EffortStep, 0.5f, strength = HapticStrength.Subtle).first().scale
        val standard = steps(Haptic.EffortStep, 0.5f).first().scale
        val strong = steps(Haptic.EffortStep, 0.5f, strength = HapticStrength.Strong).first().scale
        assertTrue(subtle < standard && standard < strong)
    }

    @Test fun nonLeveledHapticsIgnoreTheLevel() {
        for (h in Haptic.entries.filter { it !in HapticTable.leveled }) {
            assertEquals("$h", HapticPlanner.plan(h, HapticStrength.Standard, caps(), 0f), HapticPlanner.plan(h, HapticStrength.Standard, caps(), 1f))
        }
    }

    @Test fun stretchIsLightAndFirmsUpTheFurtherItIsPulled() {
        var last = 0f
        for (level in levels) {
            val first = steps(Haptic.Stretch, level).first()
            assertEquals(C.PRIMITIVE_TICK, first.primitive)
            assertTrue(first.scale > last)
            last = first.scale
        }
        assertTrue(steps(Haptic.Stretch, 0f).first().scale <= 0.3f)
        assertTrue(steps(Haptic.Stretch, 1f).first().scale <= 0.75f) // resistance, never a thunk
        assertEquals(1, steps(Haptic.Stretch, 0.2f).size)
        assertEquals(2, steps(Haptic.Stretch, 1f).size) // a low drag joins from the middle on
        assertTrue(wave(Haptic.Stretch, 1f).amplitudes!!.max() > wave(Haptic.Stretch, 0f).amplitudes!!.max())
        // Light class: coalesced and rate-capped like the other scroll-weight haptics.
        assertEquals(0, HapticTable.spec(Haptic.Stretch).priority)
        // Hardware with only LOW_TICK still gets a stretch.
        assertTrue(HapticPlanner.plan(Haptic.Stretch, HapticStrength.Standard, caps(sdk = 31, primitives = setOf(C.PRIMITIVE_LOW_TICK), view = false), 1f) is HapticPlan.Composition)
    }

    @Test fun reboundIsOneFirmThumpThatDecaysAtOnce() {
        val s = steps(Haptic.Rebound)
        assertEquals(C.PRIMITIVE_THUD, s.first().primitive)
        assertTrue(s.first().scale >= 0.8f)
        assertTrue(s.drop(1).all { it.scale < s.first().scale / 2 })
        assertTrue(duration(s) <= 40)
        val w = wave(Haptic.Rebound)
        val amps = w.amplitudes!!.filter { it > 0 }
        assertEquals(amps.sortedDescending(), amps) // decays
        assertTrue(amps.first() >= 200)
        assertTrue(w.timings.sum() <= 60)
    }

    @Test fun surgeIsATwoFiftyMillisecondCrescendoEndingInAHardCrack() {
        val s = steps(Haptic.Surge)
        val ramp = s.dropLast(2)
        assertTrue(ramp.size >= 5 && ramp.all { it.primitive == C.PRIMITIVE_TICK })
        assertEquals(ramp.map { it.scale }.sorted(), ramp.map { it.scale }) // firms up
        val gaps = ramp.drop(1).map { it.delayMs }
        assertEquals(gaps.sortedDescending(), gaps) // and bunches together
        assertEquals(C.PRIMITIVE_CLICK, s[s.size - 2].primitive)
        assertEquals(C.PRIMITIVE_THUD, s.last().primitive)
        assertEquals(1f, s.last().scale, 0f)
        assertTrue(s[s.size - 2].scale > ramp.last().scale)
        val total = duration(s) + s.size * 10 // each primitive plays about ten milliseconds
        assertTrue("about 250 ms, was $total", total in 200..320)

        // Without THUD the crack is a double click; with only CLICK it is a click crescendo.
        val noThud = steps(Haptic.Surge, c = caps(sdk = 31, primitives = setOf(C.PRIMITIVE_CLICK, C.PRIMITIVE_TICK), view = false))
        assertEquals(C.PRIMITIVE_CLICK, noThud.last().primitive)
        val clicksOnly = steps(Haptic.Surge, c = caps(sdk = 31, primitives = setOf(C.PRIMITIVE_CLICK), view = false))
        assertTrue(clicksOnly.all { it.primitive == C.PRIMITIVE_CLICK })
        assertEquals(clicksOnly.map { it.scale }.sorted(), clicksOnly.map { it.scale })
        assertEquals(1f, clicksOnly.last().scale, 0f)
    }

    @Test fun surgeWaveformFallbackCrescendos() {
        val w = wave(Haptic.Surge)
        val on = w.timings.indices.filter { w.amplitudes!![it] > 0 }.map { w.amplitudes!![it] }
        assertEquals(on.sorted(), on)
        assertEquals(255, on.last())
        assertTrue(on.first() <= 60)
        assertTrue("about 250 ms, was ${w.timings.sum()}", w.timings.sum() in 230..320)
    }

    @Test fun zipIsAVeryShortSharpDoubleTick() {
        val s = steps(Haptic.Zip)
        assertEquals(2, s.size)
        assertTrue(s.all { it.primitive == C.PRIMITIVE_TICK && it.scale >= 0.85f })
        assertTrue(s[1].delayMs in 15..40)
        val w = wave(Haptic.Zip)
        assertTrue(w.timings.sum() <= 40)
        assertTrue(w.amplitudes!!.filter { it > 0 }.all { it >= 150 })
    }

    @Test fun lightningIsAnIrregularCrackleWithAFinalCrack() {
        val s = steps(Haptic.Lightning)
        val pulses = s.dropLast(1)
        assertTrue("3 to 5 micro-pulses, was ${pulses.size}", pulses.size in 3..5)
        assertEquals(C.PRIMITIVE_CLICK, s.last().primitive)
        assertEquals(1f, s.last().scale, 0f)
        assertTrue(pulses.all { it.scale < 1f })
        // Uneven in both strength and spacing.
        assertTrue(pulses.map { it.scale }.toSet().size == pulses.size)
        assertTrue(s.drop(1).map { it.delayMs }.toSet().size == s.size - 1)
        assertFalse(pulses.map { it.scale } == pulses.map { it.scale }.sorted()) // not a clean ramp
        val total = duration(s) + s.size * 10
        assertTrue("about 180 ms, was $total", total in 150..230)
        val w = wave(Haptic.Lightning)
        assertTrue(w.timings.sum() in 130..230)
        assertEquals(255, w.amplitudes!!.max())
        assertTrue(w.amplitudes!!.last() == 255)
    }

    @Test fun railTickIsLighterThanTheSliderTick() {
        assertTrue(steps(Haptic.RailTick).first().scale < HapticTable.spec(Haptic.Tick).compositions.first().first().scale)
        assertTrue(wave(Haptic.RailTick).amplitudes!!.max() < wave(Haptic.Tick).amplitudes!!.max())
        assertEquals(0, HapticTable.spec(Haptic.RailTick).priority)
        // Standard with a view: the platform's own faintest tick.
        assertEquals(
            HapticPlan.ViewConstant(android.view.HapticFeedbackConstants.SEGMENT_FREQUENT_TICK),
            HapticPlanner.plan(Haptic.RailTick, HapticStrength.Standard, caps(sdk = 34)),
        )
    }

    @Test fun roundTwoHapticsAreNoLongerBorrowedPlaceholders() {
        val borrowed = listOf(Haptic.Select, Haptic.Tick, Haptic.Heavy, Haptic.Success).map { HapticTable.spec(it) }
        for (h in listOf(Haptic.EffortStep, Haptic.Stretch, Haptic.Rebound, Haptic.Surge, Haptic.Zip, Haptic.Lightning, Haptic.RailTick)) {
            val spec = HapticTable.spec(h)
            assertEquals(h, spec.haptic)
            for (b in borrowed) assertFalse("$h", spec.compositions == b.compositions && spec.waveform == b.waveform)
        }
    }
}
