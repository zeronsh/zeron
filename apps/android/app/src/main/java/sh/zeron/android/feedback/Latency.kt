package sh.zeron.android.feedback

import java.nio.ByteBuffer
import java.nio.ByteOrder
import kotlin.math.roundToInt

/*
 * The pure parts of the tap-to-audible path. See "Latency" in
 * docs/sound-design/android.md for the whole story; the short version is that
 * a SoundPool cue only gets the low-latency (FAST) mixer path when its sample
 * rate equals the device's output rate, and the output stays asleep (a cold
 * start costs tens of ms) unless something keeps it awake.
 */

/** A value re-read at most every [ttlMs]: the system queries behind the gate are binder calls, too slow to make per event. */
class TtlValue<T : Any>(
    private val ttlMs: Long,
    private val clock: () -> Long,
    private val read: () -> T,
) {
    @Volatile private var value: T? = null
    @Volatile private var readAt = Long.MIN_VALUE / 2

    fun get(): T {
        val now = clock()
        val cached = value
        if (cached != null && now - readAt < ttlMs) return cached
        val fresh = read()
        value = fresh
        readAt = now
        return fresh
    }

    /** The next [get] reads again (a broadcast said the underlying state changed). */
    fun invalidate() {
        readAt = Long.MIN_VALUE / 2
    }
}

/** Where the cue assets stand against the device's output. */
object OutputPath {
    /** The rate the committed `fx_*.wav` assets are generated at. */
    const val ASSET_RATE = 48_000

    /** Fallback when the platform does not report one (every current phone's mixer runs at 48 kHz). */
    const val DEFAULT_RATE = 48_000

    /** `AudioManager.PROPERTY_OUTPUT_SAMPLE_RATE` is a decimal string; anything unusable reads as [DEFAULT_RATE]. */
    fun parseRate(property: String?): Int = property?.trim()?.toIntOrNull()?.takeIf { it in 8_000..384_000 } ?: DEFAULT_RATE

    /** Assets at [ASSET_RATE] need no resampling in the mixer only when it runs at the same rate. */
    fun needsResampling(outputRate: Int): Boolean = outputRate != ASSET_RATE
}

/** 16-bit PCM WAV in memory: just enough RIFF to read the committed assets and write resampled copies. */
class Wav(val rate: Int, val channels: Int, val samples: ShortArray) {
    val frames: Int get() = samples.size / channels

    fun toBytes(): ByteArray {
        val data = samples.size * 2
        val out = ByteBuffer.allocate(44 + data).order(ByteOrder.LITTLE_ENDIAN)
        out.put("RIFF".toByteArray()).putInt(36 + data).put("WAVE".toByteArray())
        out.put("fmt ".toByteArray()).putInt(16).putShort(1).putShort(channels.toShort())
        out.putInt(rate).putInt(rate * channels * 2).putShort((channels * 2).toShort()).putShort(16)
        out.put("data".toByteArray()).putInt(data)
        for (s in samples) out.putShort(s)
        return out.array()
    }

    companion object {
        /** Null when [bytes] is not 16-bit PCM WAV (the caller then falls back to the original file). */
        fun parse(bytes: ByteArray): Wav? {
            if (bytes.size < 12) return null
            val b = ByteBuffer.wrap(bytes).order(ByteOrder.LITTLE_ENDIAN)
            if (String(bytes, 0, 4) != "RIFF" || String(bytes, 8, 4) != "WAVE") return null
            var pos = 12
            var rate = 0
            var channels = 0
            while (pos + 8 <= bytes.size) {
                val id = String(bytes, pos, 4)
                val size = b.getInt(pos + 4)
                val body = pos + 8
                if (size < 0) return null
                when (id) {
                    "fmt " -> {
                        if (body + 16 > bytes.size || b.getShort(body).toInt() != 1 || b.getShort(body + 14).toInt() != 16) return null
                        channels = b.getShort(body + 2).toInt()
                        rate = b.getInt(body + 4)
                    }
                    "data" -> {
                        if (rate <= 0 || channels !in 1..2) return null
                        val length = minOf(size, bytes.size - body) / 2 / channels * channels
                        return Wav(rate, channels, ShortArray(length) { b.getShort(body + it * 2) })
                    }
                }
                pos = body + size + (size and 1)
            }
            return null
        }
    }
}

/** Catmull-Rom resampling for the rare device whose mixer does not run at the assets' rate. */
object Resampler {
    fun resample(wav: Wav, toRate: Int): Wav {
        if (wav.rate == toRate) return wav
        val ch = wav.channels
        val inFrames = wav.frames
        if (inFrames == 0) return Wav(toRate, ch, ShortArray(0))
        val outFrames = (inFrames.toLong() * toRate / wav.rate).toInt().coerceAtLeast(1)
        val step = wav.rate.toDouble() / toRate
        val out = ShortArray(outFrames * ch)
        fun at(frame: Int, c: Int): Double = wav.samples[frame.coerceIn(0, inFrames - 1) * ch + c].toDouble()
        for (i in 0 until outFrames) {
            val pos = i * step
            val i1 = pos.toInt()
            val t = pos - i1
            for (c in 0 until ch) {
                val p0 = at(i1 - 1, c)
                val p1 = at(i1, c)
                val p2 = at(i1 + 1, c)
                val p3 = at(i1 + 2, c)
                val v = 0.5 * (2 * p1 + (-p0 + p2) * t + (2 * p0 - 5 * p1 + 4 * p2 - p3) * t * t + (-p0 + 3 * p1 - 3 * p2 + p3) * t * t * t)
                out[i * ch + c] = v.roundToInt().coerceIn(-32768, 32767).toShort()
            }
        }
        // Keep the files' clean ends: the first and last frame stay exactly as authored (zero).
        for (c in 0 until ch) {
            out[c] = wav.samples[c]
            out[(outFrames - 1) * ch + c] = wav.samples[(inFrames - 1) * ch + c]
        }
        return Wav(toRate, ch, out)
    }

    fun resampleBytes(bytes: ByteArray, toRate: Int): ByteArray? = Wav.parse(bytes)?.let { resample(it, toRate).toBytes() }
}

/**
 * When the output is kept awake. A sound output goes to standby a few seconds
 * after the last stream ends, and waking it costs tens of milliseconds on
 * many phones: the "slightly delayed" first sound after a pause. A silent
 * looped stream keeps it running, but only while the user is actually
 * touching the app (a touch-down starts it, so it is up before the click
 * that sounds), and for [IDLE_MS] after the last touch or cue.
 */
class WarmPolicy(private val clock: () -> Long, private val idleMs: Long = IDLE_MS) {
    private var until = Long.MIN_VALUE / 2

    /** Something happened that may soon want a sound (a touch-down, a cue). */
    @Synchronized
    fun touch() {
        until = clock() + idleMs
    }

    @Synchronized
    fun wanted(): Boolean = clock() < until

    /** Milliseconds until [wanted] turns false (0 when it already is). */
    @Synchronized
    fun remainingMs(): Long = (until - clock()).coerceAtLeast(0)

    @Synchronized
    fun stop() {
        until = Long.MIN_VALUE / 2
    }

    companion object {
        const val IDLE_MS = 12_000L
    }
}
