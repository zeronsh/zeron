package sh.zeron.android.feedback

import android.content.Context
import android.media.AudioAttributes
import android.media.AudioManager
import android.media.SoundPool
import android.util.Log
import java.io.File
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicInteger

/**
 * Every cue preloaded into one [SoundPool]: no audio focus (UI sounds mix
 * with the user's music), on the sonification usage so they follow the system
 * sound volume and silent mode.
 *
 * Built for the shortest tap-to-audible path (docs/sound-design/android.md,
 * "Latency"):
 *  - the attributes ask for the low-latency output, which needs the asset
 *    rate to equal the device's output rate; when it does not, the assets are
 *    resampled once into the cache ([OutputPath], [Resampler]) so the mixer
 *    never has to;
 *  - every stream's track is created ahead of the first cue ([prime]), and a
 *    silent looped stream keeps the output out of standby while the user is
 *    touching the app ([keepWarm]);
 *  - [play] is one synchronous call, with no thread hop.
 */
class SoundBank(private val context: Context) {
    private val pool: SoundPool = SoundPool.Builder()
        .setMaxStreams(FeedbackGate.MAX_STREAMS + 1) // one more than the gate allows: the keep-alive loop owns it
        .setAudioAttributes(attributes())
        .build()
    private val ids = ConcurrentHashMap<Cue, Int>()
    private val ready = ConcurrentHashMap.newKeySet<Int>()
    private val pending = AtomicInteger()

    @Volatile private var keepAliveId = 0
    @Volatile private var keepAliveStream = 0

    /** The device mixer's rate (`PROPERTY_OUTPUT_SAMPLE_RATE`); the assets are 48 kHz. */
    val outputRate: Int = OutputPath.parseRate(
        context.getSystemService(AudioManager::class.java)?.getProperty(AudioManager.PROPERTY_OUTPUT_SAMPLE_RATE),
    )

    init {
        pool.setOnLoadCompleteListener { _, sampleId, status ->
            if (status == 0) ready.add(sampleId) else Log.w(AndroidFeedback.TAG, "sound $sampleId failed to load: $status")
            if (pending.decrementAndGet() == 0) prime()
        }
    }

    /** Starts decoding every cue; returns at once (SoundPool loads asynchronously). Call off the main thread. */
    fun load() {
        val dir = if (OutputPath.needsResampling(outputRate)) resampledDir() else null
        val names: List<Pair<Cue?, String>> = CueTable.all.map { it.cue to it.resource } + (null to KEEP_ALIVE)
        pending.set(names.size)
        for ((cue, name) in names) {
            val id = loadOne(name, dir)
            if (id == 0) {
                Log.w(AndroidFeedback.TAG, "no resource $name for $cue")
                if (pending.decrementAndGet() == 0) prime()
            } else if (cue != null) {
                ids[cue] = id
            } else {
                keepAliveId = id
            }
        }
    }

    private fun loadOne(name: String, dir: File?): Int {
        val res = context.resources.getIdentifier(name, "raw", context.packageName)
        if (res == 0) return 0
        if (dir != null) {
            val file = File(dir, "$name.wav")
            if (!file.isFile || file.length() == 0L) {
                val resampled = runCatching {
                    context.resources.openRawResource(res).use { Resampler.resampleBytes(it.readBytes(), outputRate) }
                }.getOrNull()
                if (resampled != null) file.writeBytes(resampled)
            }
            if (file.isFile && file.length() > 0L) return pool.load(file.path, 1)
        }
        return pool.load(context, res, 1)
    }

    /** The cache of assets at [outputRate]; older rates and app versions are dropped. */
    private fun resampledDir(): File {
        val updated = runCatching { context.packageManager.getPackageInfo(context.packageName, 0).lastUpdateTime }.getOrDefault(0L)
        val name = "sounds-$outputRate-$updated"
        context.cacheDir.listFiles { f -> f.isDirectory && f.name.startsWith("sounds-") && f.name != name }?.forEach { it.deleteRecursively() }
        return File(context.cacheDir, name).also { it.mkdirs() }
    }

    /**
     * Once everything is decoded: create each stream's AudioTrack now, silently (a track is created at a stream's
     * first play, which costs a few ms the first cue would otherwise pay). Four overlapping plays of the
     * shortest cue reach four different streams.
     */
    private fun prime() {
        val id = ids[Cue.Tap]?.takeIf { it in ready } ?: return
        repeat(FeedbackGate.MAX_STREAMS) { pool.play(id, PRIME_VOLUME, PRIME_VOLUME, 0, 0, 1f) }
    }

    fun isReady(cue: Cue): Boolean = ids[cue]?.let { it in ready } == true

    /** [rate] resamples (pitch); [volume] is 0..1. Returns whether a stream started. */
    fun play(cue: Cue, volume: Float, rate: Float, priority: Int): Boolean {
        val id = ids[cue]?.takeIf { it in ready } ?: return false
        return pool.play(id, volume, volume, priority, 0, rate) != 0
    }

    /**
     * Starts or stops the silent looped stream that keeps the output awake. Returns whether it is running after
     * the call (false before the silence has loaded: the caller tries again on the next touch).
     */
    @Synchronized
    fun keepWarm(on: Boolean): Boolean {
        if (!on) {
            if (keepAliveStream != 0) pool.stop(keepAliveStream)
            keepAliveStream = 0
            return false
        }
        if (keepAliveStream != 0) return true
        val id = keepAliveId.takeIf { it != 0 && it in ready } ?: return false
        keepAliveStream = pool.play(id, 1f, 1f, 0, -1, 1f)
        return keepAliveStream != 0
    }

    fun release() = pool.release()

    companion object {
        /** `res/raw/silence_keepalive.wav`: 100 ms of digital silence, looped (not an `fx_` file: it is not a cue). */
        const val KEEP_ALIVE = "silence_keepalive"

        /** Inaudible (-60 dB) but not zero, so no layer treats the priming plays as muted and skips the track. */
        const val PRIME_VOLUME = 0.001f

        // USAGE_GAME, not USAGE_ASSISTANCE_SONIFICATION: the sonification usage is the OS's system-sound path, which
        // Samsung's screen recorder (and other capture / routing modes) silences while it records, on the phone and in
        // the recording alike. Game sound effects follow the media volume, are never muted by the capture, and are
        // included in a recording's "media sounds". No audio focus is requested, so music keeps playing.
        // FLAG_LOW_LATENCY is deprecated since 29 in favour of AudioTrack performance modes, which SoundPool cannot
        // take; the audio policy still reads it and routes the stream to the fast output.
        @Suppress("DEPRECATION")
        private fun attributes(): AudioAttributes = AudioAttributes.Builder()
            .setUsage(AudioAttributes.USAGE_GAME)
            .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION)
            .setFlags(AudioAttributes.FLAG_LOW_LATENCY)
            .setAllowedCapturePolicy(AudioAttributes.ALLOW_CAPTURE_BY_ALL)
            .build()
    }
}
