package sh.zeron.runtime

import android.content.Context
import android.os.Build
import android.provider.Settings
import android.system.ErrnoException
import android.system.Os
import android.system.OsConstants
import java.io.File

internal const val EDGE_PORT = 27655
internal const val IPC_PORT = 27654
internal const val GUEST_HOME = "/home/zeron"

/** Where the engine clones and creates projects (`ZERON_PROJECTS_DIR`). */
const val PROJECTS_ROOT = "$GUEST_HOME/projects"

/** Host-side layout of the runtime (docs/android.md § Runtime contract). */
internal class RuntimePaths(context: Context) {
    val nativeLibDir = File(context.applicationInfo.nativeLibraryDir)
    val root = File(context.filesDir, "runtime")
    val rootfs = File(root, "rootfs")
    val tmp = File(root, "tmp")
    val stateFile = File(root, "state.json")
    val logDir = File(root, "logs")
    val enginePidFile = File(root, "engine.pid")
    val proot = File(nativeLibDir, "libproot.so")
    val engine = File(nativeLibDir, "libzeron.so")

    /**
     * The ABI the package manager actually installed, read off nativeLibraryDir
     * (…/lib/arm64 or …/lib/x86_64) — on an x86_64 device with ARM translation
     * SUPPORTED_ABIS lists both, but only one set of executables is on disk.
     */
    val abi: String? = when (nativeLibDir.name) {
        "arm64" -> "arm64-v8a"
        "x86_64" -> "x86_64"
        else -> Build.SUPPORTED_ABIS.firstOrNull { it == "arm64-v8a" || it == "x86_64" }
    }

    /**
     * The rootfs asset in this APK. AGP's asset merge gunzips `*.gz`, so the
     * staged rootfs-<abi>.tar.gz usually ships as a plain .tar (still
     * deflated by the zip); accept either.
     */
    fun rootfsAsset(context: Context): String? {
        val abi = abi ?: return null
        val names = context.assets.list("").orEmpty()
        return listOf("rootfs-$abi.tar.gz", "rootfs-$abi.tar").firstOrNull { it in names }
    }
}

/** Builds proot invocations for the engine, exec() and bootstrap steps. */
internal class Guest(private val context: Context, private val paths: RuntimePaths) {
    val deviceName: String by lazy {
        Settings.Global.getString(context.contentResolver, Settings.Global.DEVICE_NAME)
            ?.takeIf { it.isNotBlank() } ?: Build.MODEL
    }

    /**
     * The engine's environment. By default it embeds the local edge (serving
     * the signed-out workspace to the app; a saved sign-in makes it a synced
     * device and leaves the edge off); a [CustomServer] joins that edge in
     * development scope as the single-tenant `local` identity instead, with
     * its own data dir: both workspaces are `local`/`local`, and sharing one
     * store would re-seed either edge with the other's devices and chats.
     */
    fun env(secrets: Secrets, server: CustomServer? = null): List<String> = listOf(
        "HOME=$GUEST_HOME",
        "USER=zeron",
        "LOGNAME=zeron",
        "SHELL=/bin/bash",
        "LANG=C.UTF-8",
        "TERM=xterm-256color",
        "TMPDIR=/tmp",
        "PATH=$GUEST_HOME/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
        "ZERON_DATA_DIR=$GUEST_HOME/${if (server == null) ".zeron" else ".zeron-dev"}",
        "ZERON_DEVICE_NAME=$deviceName",
        "ZERON_DEVICE_PLATFORM=android",
        "ZERON_NO_LOGIN_SHELL=1",
        "ZERON_PROJECTS_DIR=$PROJECTS_ROOT",
        "ZERON_IPC_PORT=$IPC_PORT",
        "ZERON_IPC_TOKEN=${secrets.ipcToken}",
        // Claude Code's documented setting for musl distros: use Alpine's
        // ripgrep (a bootstrap package) instead of its bundled glibc one.
        "USE_BUILTIN_RIPGREP=0",
        // The log is a file shown in the app, not a terminal: without this
        // tracing (and the CLIs it spawns) write ANSI colour escapes into it.
        "NO_COLOR=1",
    ) + if (server == null) {
        listOf("ZERON_LOCAL_EDGE_PORT=$EDGE_PORT", "ZERON_LOCAL_EDGE_TOKEN=${secrets.edgeToken}")
    } else {
        listOf(
            "ZERON_EDGE_URL=${server.edgeUrl}",
            "ZERON_EDGE_TOKEN=${server.token}",
            "ZERON_USER_ID=local",
            "ZERON_ORG_ID=local",
        )
    }

    /**
     * The full host argv. `-0` (fake root) is for bootstrap package installs
     * only: Claude Code refuses permission bypass as uid 0.
     */
    fun command(argv: List<String>, env: List<String>, asRoot: Boolean = false): List<String> =
        buildList {
            add(paths.proot.path)
            add("--kill-on-exit")
            add("--link2symlink")
            if (asRoot) add("-0")
            addAll(listOf("-r", paths.rootfs.path, "-w", if (asRoot) "/" else GUEST_HOME))
            for (bind in binds()) addAll(listOf("-b", bind))
            addAll(listOf("/usr/bin/env", "-i"))
            addAll(env)
            addAll(argv)
        }

    private fun binds(): List<String> = buildList {
        addAll(listOf("/dev", "/proc", "/sys", "/dev/urandom:/dev/random"))
        // Android has no /dev/shm or /dev/fd; node, python and bash process
        // substitution expect both.
        add("${paths.tmp.path}:/dev/shm")
        add("/proc/self/fd:/dev/fd")
        add("${paths.nativeLibDir.path}:/opt/zeron/lib")
        add("${paths.tmp.path}:/tmp")
        // Untrusted apps can't read these (SELinux); libuv's os.cpus() and
        // procps need something plausible.
        for (fake in FakeProc.files(paths)) add("${fake.path}:/proc/${fake.name}")
    }

    /**
     * Starts proot through `sh -c 'echo $$; exec …'` so the first output line is
     * proot's pid — java.lang.Process has no pid() before API 33, and the
     * engine's pid is what stop() signals.
     */
    fun start(command: List<String>): Pair<Process, Int> {
        val pb = ProcessBuilder(listOf("/system/bin/sh", "-c", "echo \$\$; exec \"\$0\" \"\$@\"") + command)
            .redirectErrorStream(true)
        pb.environment().apply {
            put("LD_LIBRARY_PATH", paths.nativeLibDir.path)
            put("PROOT_LOADER", File(paths.nativeLibDir, "libproot-loader.so").path)
            File(paths.nativeLibDir, "libproot-loader32.so").takeIf { it.exists() }
                ?.let { put("PROOT_LOADER_32", it.path) }
            put("PROOT_TMP_DIR", paths.tmp.path)
        }
        paths.tmp.mkdirs()
        val process = pb.start()
        process.outputStream.close()
        val pidLine = readLine(process.inputStream)
        val pid = pidLine.trim().toIntOrNull()
        if (pid == null) {
            process.destroy()
            error("proot launcher printed no pid: '$pidLine'")
        }
        return process to pid
    }

    // Byte-at-a-time so nothing past the pid line is buffered away from the
    // caller's reader.
    private fun readLine(input: java.io.InputStream): String {
        val sb = StringBuilder()
        while (true) {
            val c = input.read()
            if (c < 0 || c == '\n'.code) break
            sb.append(c.toChar())
        }
        return sb.toString()
    }
}

/** Static stand-ins for /proc files Android hides from apps. */
internal object FakeProc {
    fun files(paths: RuntimePaths): List<File> {
        val dir = File(paths.root, "proc")
        return NAMES.map { File(dir, it) }.filter { it.exists() }
    }

    /** Writes stand-ins only for files the app really can't read. */
    fun prepare(paths: RuntimePaths) {
        val dir = File(paths.root, "proc").apply { mkdirs() }
        val cpus = Runtime.getRuntime().availableProcessors()
        val contents = mapOf(
            "stat" to buildString {
                append("cpu  1000 0 1000 100000 0 0 0 0 0 0\n")
                repeat(cpus) { append("cpu$it 100 0 100 10000 0 0 0 0 0 0\n") }
                append("intr 0\nctxt 0\nbtime ${System.currentTimeMillis() / 1000 - 3600}\n")
                append("processes 1\nprocs_running 1\nprocs_blocked 0\nsoftirq 0 0 0 0 0 0 0 0 0 0 0\n")
            },
            "loadavg" to "0.10 0.10 0.10 1/100 100\n",
            "uptime" to "3600.00 3600.00\n",
            "vmstat" to "nr_free_pages 100000\n",
        )
        for ((name, text) in contents) {
            val file = File(dir, name)
            if (readable("/proc/$name")) file.delete() else file.writeText(text)
        }
    }

    private fun readable(path: String) = try {
        File(path).inputStream().use { it.read() }
        true
    } catch (_: Exception) {
        false
    }

    private val NAMES = listOf("stat", "loadavg", "uptime", "vmstat")
}

/** /proc-based process tree helpers; java.lang.Process only knows its child. */
internal object ProcessTree {
    fun isAlive(pid: Int) = File("/proc/$pid").exists()

    fun cmdline(pid: Int): String = try {
        File("/proc/$pid/cmdline").readText().replace('\u0000', ' ').trim()
    } catch (_: Exception) {
        ""
    }

    fun children(pid: Int): List<Int> = parents().filterValues { it == pid }.keys.toList()

    /** All descendants of [pid], deepest last. */
    fun descendants(pid: Int): List<Int> {
        val byParent = parents().entries.groupBy({ it.value }, { it.key })
        val out = ArrayList<Int>()
        val queue = ArrayDeque(listOf(pid))
        while (queue.isNotEmpty()) {
            for (child in byParent[queue.removeFirst()].orEmpty()) {
                out += child
                queue += child
            }
        }
        return out
    }

    fun signal(pid: Int, signal: Int) {
        try {
            Os.kill(pid, signal)
        } catch (_: ErrnoException) {
        }
    }

    /** SIGKILLs [pid] and everything under it, leaves first. */
    fun killTree(pid: Int) {
        for (p in descendants(pid).reversed()) signal(p, OsConstants.SIGKILL)
        signal(pid, OsConstants.SIGKILL)
    }

    private fun parents(): Map<Int, Int> {
        val out = HashMap<Int, Int>()
        for (name in File("/proc").list().orEmpty()) {
            val pid = name.toIntOrNull() ?: continue
            val stat = try {
                File("/proc/$pid/stat").readText()
            } catch (_: Exception) {
                continue
            }
            // comm may contain spaces/parens; fields resume after the last ')'.
            val ppid = stat.substringAfterLast(')').trim().split(' ').getOrNull(1)?.toIntOrNull()
            if (ppid != null) out[pid] = ppid
        }
        return out
    }
}
