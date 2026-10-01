package sh.zeron.runtime

import android.app.Activity
import android.content.Intent
import android.graphics.Typeface
import android.os.Build
import android.os.Bundle
import android.os.SystemClock
import android.util.Log
import android.view.View
import android.view.Window
import android.view.WindowInsets
import android.view.ViewGroup.LayoutParams.MATCH_PARENT
import android.view.ViewGroup.LayoutParams.WRAP_CONTENT
import android.widget.Button
import android.widget.CheckBox
import android.widget.EditText
import android.widget.HorizontalScrollView
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/**
 * Debug-only console for the runtime: state, log tail, start/stop/reset and
 * one-off guest commands. Plain Views so the runtime module needs no Compose.
 *
 * Scriptable from adb (each extra is optional; they run in this order):
 * ```
 * A=sh.zeron.android/sh.zeron.runtime.RuntimeDebugActivity
 * adb shell am start -n $A --ez start true            # [--ez reset true] wipes first
 * adb shell am start -n $A --es exec "'node --version'" [--ez root true] [--el timeout 900000]
 * adb shell am start -n $A --ez stop true
 * adb shell run-as sh.zeron.android cat files/debug-exec.log
 * ```
 * Exec results (exit code, wall time, output) are appended to
 * `filesDir/debug-exec.log` — outside runtime/, so reset() doesn't eat them —
 * and summarised in logcat under `ZeronRuntimeDebug`.
 */
class RuntimeDebugActivity : Activity() {
    private val ui = MainScope()
    private lateinit var runtime: RuntimeController
    private lateinit var stateView: TextView
    private lateinit var outputView: TextView
    private lateinit var command: EditText
    private lateinit var asRoot: CheckBox
    private lateinit var scroller: ScrollView
    private var showingLog = true

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        runtime = ZeronRuntime.get(this)
        requestWindowFeature(Window.FEATURE_NO_TITLE)
        setContentView(layout())
        ui.launch { runtime.state.collect { stateView.text = describe(it) } }
        // The log grows while the engine runs; poll it while it's on screen.
        ui.launch {
            while (isActive) {
                if (showingLog) showLog()
                delay(2_000)
            }
        }
        handle(intent)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handle(intent)
    }

    override fun onDestroy() {
        ui.cancel()
        super.onDestroy()
    }

    private fun handle(intent: Intent?) {
        val extras = intent?.extras ?: return
        if (extras.getBoolean(EXTRA_RESET)) {
            // start() must not race the wipe.
            detached.launch {
                runtime.reset()
                withContext(Dispatchers.Main) { if (extras.getBoolean(EXTRA_START)) runtime.start() }
            }
        } else if (extras.getBoolean(EXTRA_START)) {
            runtime.start()
        }
        extras.getString(EXTRA_EXEC)?.let { cmd ->
            runExec(cmd, extras.getBoolean(EXTRA_ROOT), extras.getLong(EXTRA_TIMEOUT, 900_000L))
        }
        if (extras.getBoolean(EXTRA_STOP)) runtime.stop()
        // Consumed: a rotation or re-delivery must not re-run the command.
        intent.replaceExtras(Bundle())
    }

    /**
     * Runs in [detached] so an adb-triggered install outlives this activity
     * (rotation, back press) and still lands in the exec log.
     */
    private fun runExec(cmd: String, root: Boolean, timeoutMs: Long) {
        showingLog = false
        outputView.text = "$ $cmd\n(running…)"
        detached.launch {
            // exec() before bootstrap has nothing to run in; wait for it.
            val installing = runtime.state.value
            if (installing is RuntimeState.Bootstrapping) {
                runtime.state.first { it !is RuntimeState.Bootstrapping }
            }
            val started = SystemClock.elapsedRealtime()
            val result = runtime.exec(cmd, asRoot = root, timeoutMs = timeoutMs)
            val secs = (SystemClock.elapsedRealtime() - started) / 1000.0
            val record = buildString {
                append("=== ${stamp()} exec${if (root) " (root)" else ""}: $cmd\n")
                append("=== exit ${result.exitCode} in ${"%.1f".format(Locale.US, secs)}s\n")
                append(result.output.trimEnd()).append("\n\n")
            }
            withContext(Dispatchers.IO) { File(filesDir, EXEC_LOG).appendText(record) }
            Log.i(TAG, "exec `$cmd` → ${result.exitCode} in ${secs}s: ${result.output.takeLast(300)}")
            withContext(Dispatchers.Main) {
                if (!isDestroyed) outputView.text = record
            }
        }
    }

    private fun showLog() {
        val atBottom = !scroller.canScrollVertically(1)
        outputView.text = runtime.logTail(300)
        // Follow the tail unless the user scrolled up to read.
        if (atBottom) scroller.post { scroller.fullScroll(View.FOCUS_DOWN) }
    }

    private fun layout(): View {
        val pad = (12 * resources.displayMetrics.density).toInt()
        stateView = TextView(this).apply { textSize = 14f; setPadding(0, 0, 0, pad) }
        outputView = TextView(this).apply {
            typeface = Typeface.MONOSPACE
            textSize = 10f
            setTextIsSelectable(true)
        }
        command = EditText(this).apply { hint = "guest command (sh -lc)"; setSingleLine() }
        asRoot = CheckBox(this).apply { text = "as fake root (-0)" }

        fun row(vararg buttons: Pair<String, () -> Unit>) = LinearLayout(this).apply {
            for ((label, action) in buttons) {
                addView(
                    Button(this@RuntimeDebugActivity).apply {
                        text = label
                        isAllCaps = false
                        setOnClickListener { action() }
                    },
                    LinearLayout.LayoutParams(0, WRAP_CONTENT, 1f),
                )
            }
        }

        val controls = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            addView(stateView)
            addView(
                row(
                    "Start" to {
                        RuntimePermissions.requestNotificationPermission(this@RuntimeDebugActivity)
                        runtime.start()
                    },
                    "Stop" to { runtime.stop() },
                    "Reset" to { detached.launch { runtime.reset() }; Unit },
                ),
            )
            addView(
                row(
                    "Log" to { showingLog = true; showLog() },
                    "Battery" to { RuntimePermissions.requestIgnoreBatteryOptimizations(this@RuntimeDebugActivity); Unit },
                    "Dev options" to { startActivity(RuntimePermissions.developerOptionsIntent()) },
                ),
            )
            addView(command)
            addView(
                LinearLayout(this@RuntimeDebugActivity).apply {
                    addView(asRoot, LinearLayout.LayoutParams(0, WRAP_CONTENT, 1f))
                    addView(Button(this@RuntimeDebugActivity).apply {
                        text = "Exec"
                        isAllCaps = false
                        setOnClickListener {
                            val cmd = command.text.toString().trim()
                            if (cmd.isNotEmpty()) runExec(cmd, asRoot.isChecked, 900_000L)
                        }
                    })
                },
            )
        }

        return LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            // targetSdk 35+ is edge-to-edge: keep clear of the status and
            // navigation bars ourselves.
            setOnApplyWindowInsetsListener { view, insets ->
                if (Build.VERSION.SDK_INT >= 30) {
                    val bars = insets.getInsets(WindowInsets.Type.systemBars() or WindowInsets.Type.displayCutout())
                    view.setPadding(pad + bars.left, pad + bars.top, pad + bars.right, pad + bars.bottom)
                } else {
                    // API 29 isn't edge-to-edge; the decor already insets us.
                    view.setPadding(pad, pad, pad, pad)
                }
                insets
            }
            addView(controls)
            addView(
                ScrollView(this@RuntimeDebugActivity).apply {
                    scroller = this
                    addView(HorizontalScrollView(this@RuntimeDebugActivity).apply { addView(outputView) })
                },
                LinearLayout.LayoutParams(MATCH_PARENT, 0, 1f),
            )
        }
    }

    private fun describe(state: RuntimeState): String = when (state) {
        RuntimeState.NotInstalled -> "Not installed"
        is RuntimeState.Bootstrapping ->
            "Bootstrapping: ${state.step}" + (state.progress?.let { " (${(it * 100).toInt()}%)" } ?: "")
        RuntimeState.Starting -> "Starting…"
        is RuntimeState.Running ->
            "Running · ${state.deviceName}\nipc :${state.ipcPort}"
        RuntimeState.Stopped -> "Stopped"
        is RuntimeState.Failed -> "Failed: ${state.reason}"
    } + "\nABI supported: ${runtime.isSupportedAbi}"

    private fun stamp() = SimpleDateFormat("yyyy-MM-dd HH:mm:ss", Locale.US).format(Date())

    companion object {
        private const val TAG = "ZeronRuntimeDebug"
        private const val EXEC_LOG = "debug-exec.log"
        private const val EXTRA_RESET = "reset"
        private const val EXTRA_START = "start"
        private const val EXTRA_STOP = "stop"
        private const val EXTRA_EXEC = "exec"
        private const val EXTRA_ROOT = "root"
        private const val EXTRA_TIMEOUT = "timeout"

        private val detached = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    }
}
