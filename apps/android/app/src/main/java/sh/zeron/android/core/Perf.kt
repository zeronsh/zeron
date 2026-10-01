package sh.zeron.android.core

import android.os.SystemClock
import android.util.Log
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.State
import androidx.compose.runtime.snapshotFlow
import androidx.compose.runtime.withFrameNanos

/**
 * Interaction timing for `adb logcat -s ZeronPerf` (scripts/android/measure-tab-switch.sh). [begin] stamps the
 * moment an interaction is asked for; [PerfFrame] reports when the frame that painted its result has been handed
 * off, i.e. the start of the next vsync after the composition that followed it. A main thread that is busy for
 * 200 ms between the two shows up as ~200 ms; an instant switch is one or two frames (16-33 ms at 60 Hz).
 */
object Perf {
    const val TAG = "ZeronPerf"

    @Volatile private var label = ""
    @Volatile private var t0 = 0L
    private var cpu0 = 0L

    /** Debug builds: sample the main thread's stack while an interaction is timed and log where the time went. */
    @Volatile var sampling = false
    @Volatile var tailMs = 250L
    private var sampler: Thread? = null
    private val inclusive = HashMap<String, Int>()
    private val self = HashMap<String, Int>()
    private var samples = 0

    /** One frame as the platform measured it: the main thread's own work, then the wait for the render thread. */
    private class Frame(val vsync: Long, val ui: Double, val sync: Double, val total: Double, val parts: String)

    private val frames = ArrayList<Frame>()
    private val handler by lazy { android.os.Handler(android.os.HandlerThread("ZeronPerfFrames").also { it.start() }.looper) }
    private var tMono = 0L

    /** Debug builds: keep per-frame metrics (UI-thread time = input + animation/recomposition + measure/layout + draw). */
    fun attach(window: android.view.Window) {
        window.addOnFrameMetricsAvailableListener({ _, m, _ ->
            fun ms(i: Int) = m.getMetric(i) / 1e6
            val ui = ms(android.view.FrameMetrics.INPUT_HANDLING_DURATION) + ms(android.view.FrameMetrics.ANIMATION_DURATION) +
                ms(android.view.FrameMetrics.LAYOUT_MEASURE_DURATION) + ms(android.view.FrameMetrics.DRAW_DURATION)
            synchronized(frames) {
                frames += Frame(m.getMetric(android.view.FrameMetrics.INTENDED_VSYNC_TIMESTAMP), ui, ms(android.view.FrameMetrics.SYNC_DURATION), ms(android.view.FrameMetrics.TOTAL_DURATION),
                    "anim %.0f layout %.0f draw %.0f".format(ms(android.view.FrameMetrics.ANIMATION_DURATION), ms(android.view.FrameMetrics.LAYOUT_MEASURE_DURATION), ms(android.view.FrameMetrics.DRAW_DURATION)))
                if (frames.size > 600) frames.subList(0, 300).clear()
            }
        }, handler)
    }

    private fun summarize(what: String, from: Long) {
        val run = synchronized(frames) {
            val out = ArrayList<Frame>()  // the switch itself: its frames, until the screen goes quiet or six have passed
            for (f in frames) {
                if (f.vsync < from - 20_000_000) continue
                if (out.size >= 6 || out.isNotEmpty() && f.vsync - out.last().vsync > 150_000_000) break
                out += f
            }
            out
        }
        if (run.isEmpty()) return
        val shown = run.joinToString(" | ") { "%.1f".format(it.ui) }
        Log.d(TAG, "%s: frames=%d main-thread ms per frame [%s] worst=%.1f sum=%.1f (render-thread sync wait worst=%.1f)".format(
            what, run.size, shown, run.maxOf { it.ui }, run.sumOf { it.ui }, run.maxOf { it.sync }))
        Log.d(TAG, "%s: first frame = %s".format(what, run.first().parts))
    }

    /** Ends the current timing after [n] frames (screens that open through navigation have no composable to ask). */
    fun finishAfterFrames(n: Int) {
        val choreographer = android.view.Choreographer.getInstance()
        fun wait(left: Int) {
            choreographer.postFrameCallback { if (left <= 1) finish("$n frames after the request") else wait(left - 1) }
        }
        wait(n)
    }

    fun begin(what: String) {
        label = what
        tMono = System.nanoTime()
        t0 = SystemClock.elapsedRealtimeNanos()
        cpu0 = android.os.Debug.threadCpuTimeNanos()
        if (sampling) startSampler()
    }

    private fun startSampler() {
        val main = android.os.Looper.getMainLooper().thread
        synchronized(inclusive) { inclusive.clear(); self.clear(); samples = 0 }
        val thread = Thread {
            while (!Thread.currentThread().isInterrupted) {
                val stack = main.stackTrace
                if (stack.isNotEmpty()) synchronized(inclusive) {
                    samples++
                    val seen = HashSet<String>()
                    for (f in stack) {
                        val key = f.className.substringAfterLast('.') + "." + f.methodName
                        if (seen.add(key)) inclusive.merge(key, 1, Int::plus)
                    }
                    val top = stack[0]
                    self.merge(top.className.substringAfterLast('.') + "." + top.methodName, 1, Int::plus)
                }
                try { Thread.sleep(1) } catch (_: InterruptedException) { return@Thread }
            }
        }
        thread.name = "ZeronPerfSampler"
        thread.isDaemon = true
        sampler = thread
        thread.start()
    }

    private fun report() {
        sampler?.interrupt()
        sampler = null
        synchronized(inclusive) {
            Log.d(TAG, "samples=$samples (~1 ms apart; compose frames dominate)")
            inclusive.entries.sortedByDescending { it.value }.take(400).forEach { Log.d(TAG, "incl %4d %s".format(it.value, it.key)) }
            self.entries.sortedByDescending { it.value }.take(15).forEach { Log.d(TAG, "self %4d %s".format(it.value, it.key)) }
        }
    }

    /** Milliseconds since [begin], or -1 when nothing is being timed. */
    fun elapsedMs(): Double = if (label.isEmpty()) -1.0 else (SystemClock.elapsedRealtimeNanos() - t0) / 1e6

    fun mark(step: String) {
        if (label.isEmpty()) return
        Log.d(TAG, "%s: %s +%.1f ms".format(label, step, elapsedMs()))
    }

    fun finish(step: String) {
        if (label.isEmpty()) return
        Log.d(TAG, "%s: %s +%.1f ms wall, %.1f ms main-thread cpu (done)".format(label, step, elapsedMs(), (android.os.Debug.threadCpuTimeNanos() - cpu0) / 1e6))
        val what = label
        val from = tMono
        label = ""
        handler.postDelayed({ summarize(what, from) }, 500)
        if (sampling) handler.postDelayed({ report() }, tailMs)  // keep sampling through the frames that follow the switch
    }
}

/** Reports the first frame after [state] changed (and the change was composed) to [Perf]. */
@Composable
fun PerfFrame(state: State<*>) {
    LaunchedEffect(Unit) {
        snapshotFlow { state.value }.collect {
            if (Perf.elapsedMs() < 0) return@collect
            Perf.mark("composed")
            withFrameNanos { }
            Perf.finish("first frame after composition")
        }
    }
}
