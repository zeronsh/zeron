package sh.zeron.runtime

import android.content.Context
import android.net.ConnectivityManager
import android.system.Os
import java.io.File
import java.io.FilterInputStream
import java.net.Inet6Address
import java.util.zip.GZIPInputStream

/**
 * Brings the guest from nothing to engine-ready. Each step records what it
 * did in state.json; bumping [BOOTSTRAP_VERSION] re-runs the package step on
 * upgrade without touching /home.
 */
internal class Bootstrap(
    private val context: Context,
    private val paths: RuntimePaths,
    private val store: StateStore,
    private val log: RuntimeLog,
    private val runner: GuestRunner,
    private val report: (step: String, progress: Float?) -> Unit,
) {
    /** A guest exists, possibly one an app update still has to upgrade. */
    val hasGuest: Boolean
        get() = File(paths.rootfs, "etc/alpine-release").exists()

    val isInstalled: Boolean
        get() = hasGuest &&
            store.read().let { it.packages == packagesKey() && it.bootstrapVersion >= BOOTSTRAP_VERSION }

    suspend fun run() {
        paths.root.mkdirs()
        paths.tmp.mkdirs()
        store.secrets()
        if (!File(paths.rootfs, "etc/alpine-release").exists()) extractRootfs()
        configure()
        val state = store.read()
        if (state.packages != packagesKey() || state.bootstrapVersion < BOOTSTRAP_VERSION) {
            installPackages(upgrade = state.packages != null)
            store.update { it.copy(packages = packagesKey(), bootstrapVersion = BOOTSTRAP_VERSION) }
        }
    }

    private fun extractRootfs() {
        val asset = paths.rootfsAsset(context) ?: error("no rootfs asset for ${paths.abi}")
        log.note("extracting $asset")
        report("Extracting Linux guest", 0f)
        val partial = File(paths.root, "rootfs.partial")
        deleteTree(partial)
        deleteTree(paths.rootfs)
        context.assets.open(asset).use { raw ->
            // available() on an asset stream is its full uncompressed length.
            val total = raw.available().toFloat().coerceAtLeast(1f)
            val counting = CountingInputStream(raw.buffered(64 * 1024))
            val tar = if (asset.endsWith(".gz")) GZIPInputStream(counting, 64 * 1024) else counting
            val progress = object : FilterInputStream(tar) {
                var last = 0f
                override fun read(b: ByteArray, off: Int, len: Int): Int = super.read(b, off, len).also {
                    val p = (counting.count / total).coerceAtMost(1f)
                    if (p - last >= 0.02f) {
                        last = p
                        report("Extracting Linux guest", p)
                    }
                }
            }
            TarExtractor(partial).extract(progress)
        }
        if (!partial.renameTo(paths.rootfs)) error("could not move the extracted rootfs into place")
        store.update { it.copy(rootfsRelease = ROOTFS_RELEASE) }
        log.note("rootfs $ROOTFS_RELEASE extracted")
    }

    /** Cheap, idempotent host-side setup; runs on every start (DNS can change). */
    fun configure() {
        val etc = File(paths.rootfs, "etc")
        writeUser(etc)
        File(etc, "resolv.conf").writeText(dnsServers().joinToString("") { "nameserver $it\n" })
        File(etc, "hosts").writeText(
            "127.0.0.1\tlocalhost localhost.localdomain\n::1\t\tlocalhost ip6-localhost ip6-loopback\n",
        )
        // /etc/profile resets PATH; login shells (exec's `sh -lc`) still need
        // ~/.local/bin, where the Claude Code installer puts `claude`.
        File(etc, "profile.d/zeron.sh").apply { parentFile?.mkdirs() }.writeText(
            """
            |case ":${'$'}PATH:" in *":${'$'}HOME/.local/bin:"*) ;; *) export PATH="${'$'}HOME/.local/bin:${'$'}PATH" ;; esac
            |export TMPDIR=/tmp
            |""".trimMargin(),
        )
        for (dir in listOf(".zeron", "projects", ".local/bin")) File(paths.rootfs, "home/zeron/$dir").mkdirs()
        File(paths.rootfs, "opt/zeron/lib").mkdirs()
        val link = File(paths.rootfs, "usr/local/bin/zeron").apply { parentFile?.mkdirs() }
        if (lstat(link) == null) Os.symlink("/opt/zeron/lib/libzeron.so", link.path)
        // Every guest file belongs to the app's uid, so `apk add` already works
        // unprivileged; agents reach for sudo out of habit, and a missing one
        // sends them down a "can't install anything" dead end.
        File(paths.rootfs, "usr/local/bin/sudo").apply {
            writeText(SUDO_SHIM)
            Os.chmod(path, 0b111_101_101)
        }
        writeAgentHints()
        FakeProc.prepare(paths)
    }

    /**
     * Global instructions each CLI reads on every session: without them
     * agents assume Debian (`apt install`) and give up on missing tools.
     * Written only when absent — the user owns these files after that.
     */
    private fun writeAgentHints() {
        for (rel in listOf(".claude/CLAUDE.md", ".codex/AGENTS.md", ".config/opencode/AGENTS.md")) {
            val file = File(paths.rootfs, "home/zeron/$rel")
            if (file.exists()) continue
            file.parentFile?.mkdirs()
            file.writeText(AGENT_HINTS)
        }
    }

    private fun writeUser(etc: File) {
        val uid = Os.getuid()
        val gid = Os.getgid()
        val passwd = File(etc, "passwd")
        val users = passwd.readLines().filterNot { it.startsWith("zeron:") }
        passwd.writeText((users + "zeron:x:$uid:$gid:Zeron:$GUEST_HOME:/bin/bash").joinToString("\n", postfix = "\n"))

        // The app also runs with Android's supplementary groups (inet, …);
        // naming them keeps `id` and friends from printing lookup errors.
        val group = File(etc, "group")
        val groups = group.readLines().filterNot { it.startsWith("zeron:") || it.startsWith("aid_") }
        val known = groups.mapNotNull { it.split(':').getOrNull(2)?.toIntOrNull() }.toSet()
        val extra = supplementaryGroups().filter { it != gid && it !in known }.distinct().map { "aid_$it:x:$it:zeron" }
        group.writeText((groups + "zeron:x:$gid:" + extra).joinToString("\n", postfix = "\n"))
    }

    // android.system.Os has no getgroups(); /proc has the same list.
    private fun supplementaryGroups(): List<Int> = try {
        File("/proc/self/status").readLines().firstOrNull { it.startsWith("Groups:") }
            ?.substringAfter(':')?.trim()?.split(Regex("\\s+"))?.mapNotNull { it.toIntOrNull() }.orEmpty()
    } catch (_: Exception) {
        emptyList()
    }

    private fun dnsServers(): List<String> {
        val cm = context.getSystemService(ConnectivityManager::class.java)
        val servers = try {
            cm?.activeNetwork?.let { cm.getLinkProperties(it) }?.dnsServers.orEmpty()
                // musl's resolv.conf has no scope ids for link-local v6.
                .filterNot { it is Inet6Address && it.isLinkLocalAddress }
                .mapNotNull { it.hostAddress }
        } catch (e: Exception) {
            emptyList()
        }
        // musl reads at most three nameservers.
        return (servers + FALLBACK_DNS).distinct().take(3)
    }

    private suspend fun installPackages(upgrade: Boolean) {
        log.note("installing packages: ${PACKAGES.joinToString(" ")}")
        report("Updating package index", null)
        runner.runChecked(listOf("/sbin/apk", "update"), asRoot = true)
        if (upgrade) {
            report("Upgrading guest packages", null)
            runner.runChecked(listOf("/sbin/apk", "upgrade", "--available"), asRoot = true, onLine = ::apkProgress)
        }
        report("Installing packages", 0f)
        runner.runChecked(listOf("/sbin/apk", "add") + PACKAGES, asRoot = true, onLine = ::apkProgress)
        log.note("packages installed")
    }

    // "(12/40) Installing nodejs (24.1.0-r0)"
    private fun apkProgress(line: String) {
        val m = APK_LINE.find(line) ?: return
        val (done, total, verb, name) = m.destructured
        report("$verb $name (${done}/$total)", done.toFloat() / total.toFloat().coerceAtLeast(1f))
    }

    private fun packagesKey() = PACKAGES.joinToString(" ")

    companion object {
        const val BOOTSTRAP_VERSION = 2
        const val ROOTFS_RELEASE = "3.24.2"
        val PACKAGES = listOf(
            "bash", "git", "nodejs", "npm", "curl", "ca-certificates", "ripgrep",
            "libgcc", "libstdc++", "openssh-client", "procps", "coreutils",
            "python3", "py3-pip", "make", "unzip", "less",
        )
        private val SUDO_SHIM = """
            |#!/bin/sh
            |# Zeron: this guest has no privilege boundary to cross (every file belongs
            |# to the app's uid), so sudo just runs the command.
            |while [ $# -gt 0 ]; do
            |  case "$1" in
            |    -u|-g|-C|-h|-p) shift 2 ;;
            |    --) shift; break ;;
            |    -*) shift ;;
            |    *) break ;;
            |  esac
            |done
            |exec "$@"
            |""".trimMargin()
        private val AGENT_HINTS = """
            |# Environment
            |
            |You are running on an Android phone, inside Zeron's Alpine Linux guest
            |(proot, no real root, no systemd, no Docker).
            |
            |- Install system packages with `apk add <pkg>` (Alpine names, e.g.
            |  `python3`, `py3-pip`, `build-base`, `go`, `rust`, `cargo`). It works
            |  without sudo; `sudo` exists but only runs the command.
            |- Python packages: prefer `python3 -m venv .venv` then pip inside it.
            |- The C library is musl, not glibc: prebuilt glibc-only binaries won't run.
            |- Projects live under ~/projects. Storage and CPU are a phone's — prefer
            |  small, incremental builds.
            |""".trimMargin()
        private val FALLBACK_DNS = listOf("1.1.1.1", "8.8.8.8")
        private val APK_LINE = Regex("""^\((\d+)/(\d+)\)\s+(\S+)\s+(\S+)""")
    }
}
