package sh.zeron.runtime

import android.content.Context
import android.content.Intent
import android.os.SystemClock
import android.system.OsConstants
import androidx.core.content.ContextCompat
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.runInterruptible
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull
import java.io.File

/**
 * The runtime behind [ZeronRuntime]: bootstrap, then the engine under proot,
 * supervised for as long as [RuntimeService] is alive.
 */
internal class GuestRuntime(private val app: Context) : RuntimeController {
    private val paths = RuntimePaths(app)
    private val log = RuntimeLog(paths.logDir)
    private val store = StateStore(paths.stateFile)
    private val guest = Guest(app, paths)
    private val runner = GuestRunner(guest, store, log)
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private val bootstrap = Bootstrap(app, paths, store, log, runner) { step, progress ->
        _state.value = RuntimeState.Bootstrapping(step, progress)
    }

    /** Serialises bootstrap and reset; exec() waits behind it. */
    private val lifecycle = Mutex()
    private var supervisor: Job? = null
    private var shutdown: Job? = null
    @Volatile private var stopping = false
    @Volatile private var serviceActive = false

    /** The running engine (proot) process, for [restart]. */
    @Volatile private var engine: Pair<Process, Int>? = null
    @Volatile private var restarting = false

    private val _state = MutableStateFlow(idleState())
    override val state: StateFlow<RuntimeState> = _state.asStateFlow()

    override val guestRootDir: File get() = paths.rootfs
    override val guestTmpDir: File get() = paths.tmp

    override val isSupportedAbi: Boolean
        get() = paths.abi != null && paths.proot.exists() && paths.engine.exists() && hasRootfsAsset()

    override fun start() {
        if (!isSupportedAbi) {
            fail("This device's CPU (${android.os.Build.SUPPORTED_ABIS.joinToString()}) isn't supported; " +
                "the on-device engine needs arm64-v8a or x86_64 and a build that bundles it.")
            return
        }
        try {
            ContextCompat.startForegroundService(app, Intent(app, RuntimeService::class.java))
        } catch (e: Exception) {
            // ForegroundServiceStartNotAllowedException: start() from the background.
            fail("Couldn't start the engine service: ${e.message}")
        }
    }

    override fun stop() {
        app.stopService(Intent(app, RuntimeService::class.java))
        // No service (e.g. after a failure): stop() acknowledges it.
        if (!serviceActive) {
            shutdown()
            if (_state.value is RuntimeState.Failed) _state.value = idleState()
        }
    }

    override fun restart() {
        val running = engine
        if (running == null || supervisor?.isActive != true) {
            start()
            return
        }
        restarting = true
        log.note("restart requested")
        scope.launch(Dispatchers.IO) { terminate(running.first, running.second) }
    }

    override var customServer: CustomServer?
        get() = store.read().customServer
        set(value) {
            store.update { it.copy(customServer = value) }
        }

    override suspend fun reset() {
        stop()
        shutdown().join()
        runner.killAll()
        lifecycle.withLock {
            withContext(Dispatchers.IO) { deleteTree(paths.root) }
            _state.value = RuntimeState.NotInstalled
        }
    }

    override fun logTail(lines: Int): String = log.tail(lines)

    override suspend fun exec(command: String, asRoot: Boolean, timeoutMs: Long): ExecResult {
        lifecycle.withLock {
            if (!File(paths.rootfs, "etc/alpine-release").exists()) {
                return ExecResult(127, "The Zeron runtime isn't installed yet; start() it first.")
            }
            // Without a live engine nobody has refreshed DNS/passwd this boot.
            if (supervisor?.isActive != true) withContext(Dispatchers.IO) { bootstrap.configure() }
        }
        log.note("exec${if (asRoot) " (root)" else ""}: $command")
        return runner.run(listOf("/bin/sh", "-lc", command), asRoot, timeoutMs)
    }

    // ── RuntimeService hooks ────────────────────────────────────────────────

    @Synchronized
    internal fun onServiceStarted() {
        serviceActive = true
        val pending = shutdown
        if (pending == null && supervisor?.isActive == true) return
        shutdown = null
        // A quick stop→start must not let the old supervisor's SIGTERM grace
        // overlap a new engine on the same ports: wait for it first.
        supervisor = scope.launch {
            pending?.join()
            stopping = false
            supervise()
        }
    }

    internal fun onServiceStopped() {
        serviceActive = false
        shutdown()
    }

    @Synchronized
    private fun shutdown(): Job = shutdown ?: supervisor.let { target ->
        scope.launch {
            stopping = true
            target?.cancelAndJoin()
            if (_state.value !is RuntimeState.Failed) _state.value = idleState()
        }
    }.also { shutdown = it }

    // ── supervision ─────────────────────────────────────────────────────────

    private suspend fun supervise() {
        try {
            lifecycle.withLock {
                withContext(Dispatchers.IO) {
                    if (bootstrap.isInstalled) bootstrap.configure() else bootstrap.run()
                }
            }
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            val detail = (e as? GuestCommandException)?.outputTail?.let { "\n$it" }.orEmpty()
            log.note("bootstrap failed: ${e.message}$detail")
            fail("Setting up the Linux guest failed: ${e.message}")
            stopService()
            return
        }
        withContext(Dispatchers.IO) { killStaleEngine() }

        var failures = 0
        while (true) {
            _state.value = RuntimeState.Starting
            val startedAt = SystemClock.elapsedRealtime()
            val exit = try {
                runEngineOnce()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                log.note("engine launch failed: ${e.message}")
                EngineExit(-1, null)
            }
            if (stopping) return
            val ranMs = SystemClock.elapsedRealtime() - startedAt
            log.note("engine exited ($exit) after ${ranMs / 1000}s")
            if (restarting) {
                // Asked for (sign-in/out): straight back up, no failure counted.
                restarting = false
                failures = 0
                continue
            }

            // The phantom process killer (and lmkd) SIGKILL children of a
            // foregrounded app; restarting would just be killed again.
            if (exit.killedBy == OsConstants.SIGKILL && serviceActive) {
                fail(PHANTOM_KILL_REASON)
                stopService()
                return
            }
            failures = if (ranMs > 60_000) 1 else failures + 1
            if (failures >= MAX_QUICK_FAILURES) {
                fail("The engine keeps exiting ($exit); see the log for details.")
                stopService()
                return
            }
            val backoff = minOf(60_000L, 1_000L shl (failures - 1))
            log.note("restarting in ${backoff / 1000}s")
            delay(backoff)
        }
    }

    /**
     * How the engine ended. proot's own exit code can't tell: when its first
     * tracee dies by a signal it exits with whatever status another tracee
     * last reported (often 0), but it does print "vpid 1: terminated with
     * signal N", which the log reader picks up. A signal on proot itself
     * (the killer may pick either process) shows up as 128+N.
     */
    private data class EngineExit(val code: Int, val signal: Int?) {
        val killedBy: Int? get() = signal ?: (code - 128).takeIf { code > 128 }
        override fun toString() = if (killedBy != null) "signal $killedBy" else "code $code"
    }

    private suspend fun runEngineOnce(): EngineExit = withContext(Dispatchers.IO) {
        val secrets = store.secrets()
        val server = store.read().customServer
        val command = guest.command(listOf("/opt/zeron/lib/libzeron.so", "headless"), guest.env(secrets, server))
        val (process, pid) = guest.start(command)
        paths.enginePidFile.writeText("$pid")
        engine = process to pid
        log.note("engine starting (proot pid $pid${server?.let { ", server ${it.edgeUrl}" }.orEmpty()})")
        var signal: Int? = null
        val reader = Thread({
            try {
                process.inputStream.bufferedReader().forEachLine { line ->
                    ENGINE_SIGNALLED.find(line)?.let { signal = it.groupValues[1].toIntOrNull() }
                    log.append(line)
                }
            } catch (_: java.io.IOException) {
            }
        }, "zeron-engine-log").apply { isDaemon = true; start() }
        val health = launch { watchHealth(secrets) }
        try {
            val code = runInterruptible { process.waitFor() }
            // proot's last words (the signal line) may still be in the pipe.
            runInterruptible { reader.join(1_000) }
            EngineExit(code, signal)
        } finally {
            health.cancel()
            engine = null
            withContext(NonCancellable) {
                if (process.isAlive) terminate(process, pid)
                paths.enginePidFile.delete()
            }
        }
    }

    /**
     * SIGTERM the engine itself (proot's first tracee) so it can flush and
     * stop its harnesses; --kill-on-exit sweeps the rest. SIGKILL the whole
     * tree if it hasn't gone in time.
     */
    private suspend fun terminate(process: Process, prootPid: Int) {
        val engines = ProcessTree.children(prootPid)
        log.note("stopping engine (pids ${engines.ifEmpty { listOf(prootPid) }.joinToString()})")
        for (pid in engines.ifEmpty { listOf(prootPid) }) ProcessTree.signal(pid, OsConstants.SIGTERM)
        val exited = withTimeoutOrNull(STOP_GRACE_MS) { runInterruptible { process.waitFor() } } != null
        if (!exited) {
            log.note("engine ignored SIGTERM for ${STOP_GRACE_MS / 1000}s; killing")
            ProcessTree.killTree(prootPid)
            runInterruptible { process.waitFor() }
        }
    }

    /** A previous app process may have died with its engine still bound to the ports. */
    private fun killStaleEngine() {
        val pid = paths.enginePidFile.takeIf { it.exists() }?.readText()?.trim()?.toIntOrNull() ?: return
        if (ProcessTree.isAlive(pid) && ProcessTree.cmdline(pid).contains("libproot.so")) {
            log.note("killing stale engine tree (proot pid $pid)")
            ProcessTree.killTree(pid)
        }
        paths.enginePidFile.delete()
    }

    /**
     * Running once the IPC server accepts our token (it binds only after the
     * engine has opened its workspace — and, signed out, its local edge).
     * Which edge the engine syncs through is the engine's to say
     * (`EdgeBearer`): the app asks it, not the runtime.
     */
    private suspend fun watchHealth(secrets: Secrets) {
        var misses = 0
        var ipcRejected = false
        while (true) {
            when (Health.ipcStatus(IPC_PORT, secrets.ipcToken)) {
                101 -> {
                    misses = 0
                    if (_state.value !is RuntimeState.Running) {
                        log.note("engine healthy (ipc :$IPC_PORT)")
                        _state.value = RuntimeState.Running(
                            ipcPort = IPC_PORT,
                            ipcToken = secrets.ipcToken,
                            deviceName = guest.deviceName,
                        )
                    }
                }
                401, 403 -> if (!ipcRejected) {
                    ipcRejected = true
                    log.note("engine IPC :$IPC_PORT rejected ZERON_IPC_TOKEN; is another engine on the port?")
                }
                else -> if (_state.value is RuntimeState.Running && ++misses >= 6) {
                    log.note("engine stopped answering health checks")
                    _state.value = RuntimeState.Starting
                    misses = 0
                }
            }
            delay(if (_state.value is RuntimeState.Running) 5_000 else 1_000)
        }
    }

    private fun stopService() {
        app.stopService(Intent(app, RuntimeService::class.java))
    }

    private fun fail(reason: String) {
        log.note("failed: $reason")
        _state.value = RuntimeState.Failed(reason, log.tail(50))
    }

    // An outdated guest is still installed: start() upgrades it in place, so
    // an app update must not send the user back through first-run setup.
    private fun idleState(): RuntimeState =
        if (bootstrap.hasGuest) RuntimeState.Stopped else RuntimeState.NotInstalled

    private fun hasRootfsAsset(): Boolean = paths.rootfsAsset(app) != null

    companion object {
        private const val STOP_GRACE_MS = 8_000L
        private const val MAX_QUICK_FAILURES = 5
        private val ENGINE_SIGNALLED = Regex("""proot info: vpid 1: terminated with signal (\d+)""")

        const val PHANTOM_KILL_REASON =
            "Android killed the engine (SIGKILL) while it was running in the foreground. This is " +
                "usually the phantom process killer: turn on Settings → System → Developer options → " +
                "\"Disable child process restrictions\" (Android 14+). On Android 12–13 run " +
                "`adb shell device_config set_sync_disabled_for_tests persistent` and " +
                "`adb shell device_config put activity_manager max_phantom_processes 2147483647`. " +
                "Very low memory can cause the same kill."
    }
}
