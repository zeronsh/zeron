package sh.zeron.android.feedback

import java.io.File
import kotlin.math.PI
import kotlin.math.abs
import kotlin.math.log10
import kotlin.math.sin
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

/** The tap-to-audible path: cached system reads, the output rate, the assets' own leading silence, the keep-warm policy. */
class LatencyTest {
    private val raw = File("src/main/res/raw")

    // --- the gate must not query the system per event

    @Test fun ttlValueReadsOncePerWindow() {
        var now = 0L
        var reads = 0
        val v = TtlValue(2_000, { now }) { ++reads }
        repeat(100) { assertEquals(1, v.get()) }
        assertEquals(1, reads)
        now = 1_999
        assertEquals(1, v.get())
        now = 2_000
        assertEquals(2, v.get())
        assertEquals(2, reads)
    }

    @Test fun ttlValueInvalidateForcesAFreshRead() {
        var reads = 0
        val v = TtlValue(60_000, { 5L }) { ++reads }
        v.get()
        v.invalidate()
        assertEquals(2, v.get())
        assertEquals(2, v.get())
    }

    // --- output rate

    @Test fun outputRateParsing() {
        assertEquals(48_000, OutputPath.parseRate("48000"))
        assertEquals(44_100, OutputPath.parseRate(" 44100 "))
        assertEquals(OutputPath.DEFAULT_RATE, OutputPath.parseRate(null))
        assertEquals(OutputPath.DEFAULT_RATE, OutputPath.parseRate("fast"))
        assertEquals(OutputPath.DEFAULT_RATE, OutputPath.parseRate("0"))
        assertFalse(OutputPath.needsResampling(48_000))
        assertTrue(OutputPath.needsResampling(44_100))
        assertTrue(OutputPath.needsResampling(96_000))
    }

    // --- resampling for the rare non-48k mixer

    private fun sine(rate: Int, hz: Double, ms: Int): Wav {
        val n = rate * ms / 1000
        val fade = rate / 200
        return Wav(rate, 1, ShortArray(n) { i ->
            val edge = minOf(i, n - 1 - i).coerceAtMost(fade) / fade.toDouble()
            (12_000 * edge * sin(2 * PI * hz * i / rate)).toInt().toShort()
        })
    }

    private fun zeroCrossings(s: ShortArray) = (1 until s.size).count { s[it - 1] < 0 && s[it] >= 0 }

    @Test fun wavRoundTrips() {
        val w = sine(48_000, 880.0, 30)
        val parsed = Wav.parse(w.toBytes())
        assertNotNull(parsed)
        assertEquals(48_000, parsed!!.rate)
        assertEquals(1, parsed.channels)
        assertArrayEquals(w.samples, parsed.samples)
        assertNull(Wav.parse(ByteArray(10)))
        assertNull(Wav.parse("RIFF....WAVEjunk".toByteArray()))
    }

    @Test fun resamplingKeepsPitchLengthAndCleanEnds() {
        for (target in listOf(44_100, 96_000, 16_000)) {
            val src = sine(48_000, 1_000.0, 100)
            val out = Resampler.resample(src, target)
            assertEquals(target, out.rate)
            assertEquals(target / 10.0, out.samples.size.toDouble(), 2.0) // 100 ms
            // 1 kHz for 100 ms is 100 cycles, whatever the rate.
            assertEquals("rate $target", 100.0, zeroCrossings(out.samples).toDouble(), 2.0)
            assertEquals(0, out.samples.first().toInt())
            assertEquals(0, out.samples.last().toInt())
            val peak = out.samples.maxOf { abs(it.toInt()) }
            assertTrue("peak $peak", peak in 11_000..12_600) // no resampling gain or clipping
        }
    }

    @Test fun resamplingToTheSameRateIsTheIdentity() {
        val w = sine(48_000, 500.0, 20)
        assertSame(w, Resampler.resample(w, 48_000))
    }

    @Test fun resamplingAnAssetEndToEnd() {
        val bytes = File(raw, "fx_tap.wav").readBytes()
        val out = Resampler.resampleBytes(bytes, 44_100)
        assertNotNull(out)
        val w = Wav.parse(out!!)!!
        assertEquals(44_100, w.rate)
        assertTrue(w.frames in 1_400..1_520) // 32 ms
    }

    // --- keep warm

    @Test fun warmPolicyFollowsTouchesAndLapses() {
        var now = 1_000L
        val warm = WarmPolicy({ now })
        assertFalse(warm.wanted())
        warm.touch()
        assertTrue(warm.wanted())
        now += WarmPolicy.IDLE_MS - 1
        assertTrue(warm.wanted())
        assertEquals(1, warm.remainingMs())
        warm.touch() // a new touch pushes the deadline out
        now += WarmPolicy.IDLE_MS - 1
        assertTrue(warm.wanted())
        now += 2
        assertFalse(warm.wanted())
        assertEquals(0, warm.remainingMs())
        warm.touch()
        warm.stop()
        assertFalse(warm.wanted())
    }

    // --- the assets themselves: leading silence is latency

    private fun committed(): List<File> = raw.listFiles { f -> f.name.startsWith("fx_") && f.name.endsWith(".wav") }!!.sortedBy { it.name }

    @Test fun everyCueStartsSoundingWithinOneMillisecond() {
        val files = committed()
        assertTrue(files.size >= 39)
        for (f in files) {
            val w = Wav.parse(f.readBytes())!!
            val peak = w.samples.maxOf { abs(it.toInt()) }
            val floor = peak * 0.01 // -40 dB re peak
            val onset = w.samples.indexOfFirst { abs(it.toInt()) >= floor }
            val ms = onset * 1000.0 / w.rate
            assertTrue("${f.name}: onset at ${"%.2f".format(ms)} ms", ms <= 1.0)
            // ...and still starts from exactly zero (no click).
            assertEquals(f.name, 0, w.samples.first().toInt())
        }
    }

    @Test fun everyCueIsMono48kAndNeverClips() {
        for (f in committed()) {
            val w = Wav.parse(f.readBytes())!!
            assertEquals(f.name, 48_000, w.rate)
            assertEquals(f.name, 1, w.channels)
            val peak = w.samples.maxOf { abs(it.toInt()) }
            val peakDb = 20 * log10(peak / 32768.0)
            // The mastering limiter's ceiling is -0.5 dBFS: loud, but no sample is anywhere near full scale.
            assertTrue("${f.name} peak $peakDb dBFS", peakDb <= -0.4)
            assertTrue("${f.name} clips", peak < 32767)
        }
    }

    /** Active-region RMS (first to last sample within 30 dB of the peak) in dBFS, as the audit measures it. */
    private fun activeRmsDb(f: File): Double {
        val x = Wav.parse(f.readBytes())!!.samples
        val floor = x.maxOf { abs(it.toInt()) } * 0.0316
        val a = x.indexOfFirst { abs(it.toInt()) >= floor }
        val b = x.indexOfLast { abs(it.toInt()) >= floor }
        var sum = 0.0
        for (i in a..b) sum += x[i].toDouble() * x[i]
        return 20 * log10(Math.sqrt(sum / (b - a + 1)) / 32768.0)
    }

    @Test fun theDefaultSliderPlaysAtThePreviousBuildsLoudestSetting() {
        // The previous build played these files (active RMS below, interface cues -38) at SoundPool volume `trim`
        // at its slider 100%. The default slider now plays the new files at `trim / ASSET_BOOST`; the audit asserts
        // the default is no quieter than that (twice the amplitude of the previous default) with an A-weighted measure as well, this is the independent JVM guard (plain RMS).
        val previousRms = mapOf(
            "fx_send" to -34.6, "fx_queued" to -36.7, "fx_upload_ready" to -35.7, "fx_reconnected" to -36.3, "fx_undo" to -36.8,
            "fx_chime_done" to -34.3, "fx_chime_request" to -35.7, "fx_chime_attention" to -35.8,
        )
        val previousTrim = mapOf("fx_send" to 0.8, "fx_queued" to 0.8, "fx_upload_ready" to 0.8, "fx_reconnected" to 0.8, "fx_undo" to 0.8, "fx_fast_on" to 0.7, "fx_fast_off" to 0.8)
        for (spec in CueTable.all) {
            val old = (previousRms[spec.resource] ?: -38.0) + 20 * log10(previousTrim[spec.resource] ?: 1.0)
            val volume = CueTable.volume(spec, FeedbackSettings().gain)
            val new = activeRmsDb(File(raw, spec.resource + ".wav")) + 20 * log10(volume.toDouble())
            assertTrue("${spec.cue}: ${"%.1f".format(new - old)} dB against the previous 100%", new - old >= -0.5)
            // And the top of the slider is another 2x (6 dB) above the default, unclipped by SoundPool's 1.0 cap.
            val top = CueTable.volume(spec, FeedbackSettings(volume = 1f).gain)
            assertEquals(spec.cue.name, 6.02, 20 * log10((top / volume).toDouble()), 0.01)
        }
    }

    @Test fun notificationChannelsPlayTheMasteredChimesAndTheChannelVersionWasBumped() {
        val sounds = sh.zeron.android.core.Notifier.Kind.entries.map { it.sound }
        // Session kinds first, then the file-transfer kinds that reuse the same three chimes.
        assertEquals(listOf("fx_chime_done", "fx_chime_request", "fx_chime_attention"), sounds.take(3))
        assertTrue(sounds.all { it in setOf("fx_chime_done", "fx_chime_request", "fx_chime_attention") })
        for (name in sounds) assertTrue(name, File(raw, "$name.wav").isFile)
        assertTrue(sh.zeron.android.core.Notifier.CHANNEL_VERSION >= 2) // channel sounds are immutable once created
    }

    @Test fun theKeepAliveLoopIsSilentAndNotACue() {
        val f = File(raw, "${SoundBank.KEEP_ALIVE}.wav")
        assertTrue(f.isFile)
        val w = Wav.parse(f.readBytes())!!
        assertEquals(48_000, w.rate)
        assertTrue(w.samples.all { it.toInt() == 0 })
        assertTrue(committed().none { it.name.startsWith(SoundBank.KEEP_ALIVE) })
    }
}
