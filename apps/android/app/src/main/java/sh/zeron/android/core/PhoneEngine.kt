package sh.zeron.android.core

import android.content.Context
import kotlinx.coroutines.flow.StateFlow
import sh.zeron.runtime.CustomServer
import sh.zeron.runtime.ExecResult
import sh.zeron.runtime.RuntimeController
import sh.zeron.runtime.RuntimeState
import sh.zeron.runtime.ZeronRuntime

/**
 * This phone's engine (`:runtime`) as the UI sees it: state, lifecycle,
 * logs and one-off guest commands (docs/android.md § Runtime API).
 */
class PhoneEngine(context: Context) {
    private val runtime: RuntimeController = ZeronRuntime.get(context)

    val state: StateFlow<RuntimeState> get() = runtime.state
    val isSupportedAbi: Boolean get() = runtime.isSupportedAbi

    fun start() = runtime.start()
    fun stop() = runtime.stop()
    fun restart() = runtime.restart()
    suspend fun reset() = runtime.reset()

    /** Developer: the edge the engine joins instead of Zeron's (next start). */
    var customServer: CustomServer?
        get() = runtime.customServer
        set(value) {
            runtime.customServer = value
        }

    /** The runtime's log with the engine's terminal colours stripped. */
    fun logTail(lines: Int = 200): String = stripAnsi(runtime.logTail(lines))
    suspend fun exec(command: String, timeoutMs: Long = 600_000): ExecResult = runtime.exec(command, timeoutMs = timeoutMs)

    /** Guest paths (what the engine reports) ↔ the app's view of the rootfs. */
    val paths: Transfers.GuestPaths get() = Transfers.GuestPaths(runtime.guestRootDir, runtime.guestTmpDir)

    companion object {
        /** Projects on the phone's engine live here (docs/android.md § Guest layout). */
        const val PROJECTS_ROOT = sh.zeron.runtime.PROJECTS_ROOT

        /**
         * Folder name a `git clone <url>` creates: the last path segment without
         * `.git` (`git@host:org/repo.git` → `repo`). `null` for nothing usable.
         */
        fun repoName(url: String): String? {
            val trimmed = url.trim().trimEnd('/').removeSuffix(".git")
            // A bare word is not a repository URL.
            if (!trimmed.contains('/') && !trimmed.contains(':')) return null
            return folderName(trimmed.substringAfterLast('/').substringAfterLast(':'))
        }

        /** A safe single folder name, or null. */
        fun folderName(name: String): String? =
            name.trim().takeIf { it.isNotEmpty() && it != "." && it != ".." && it.all { c -> c.isLetterOrDigit() || c in "._-" } }

        private val ansi = Regex("\u001B\\[[0-9;?]*[A-Za-z]")

        fun stripAnsi(s: String): String = s.replace(ansi, "")

        /** Single-quote a string for `sh -c`. */
        fun shellQuote(s: String): String = "'" + s.replace("'", "'\\''") + "'"

        fun stateLabel(s: RuntimeState): String = when (s) {
            RuntimeState.NotInstalled -> "Not set up"
            is RuntimeState.Bootstrapping -> "Setting up"
            RuntimeState.Starting -> "Starting"
            is RuntimeState.Running -> "Running"
            RuntimeState.Stopped -> "Stopped"
            is RuntimeState.Failed -> "Stopped with an error"
        }
    }
}
