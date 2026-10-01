package sh.zeron.runtime

import android.content.Context
import kotlinx.coroutines.flow.StateFlow
import java.io.File

/** Entry point to the on-device engine runtime (docs/android.md § Runtime API). */
object ZeronRuntime {
    @Volatile private var instance: GuestRuntime? = null

    fun get(context: Context): RuntimeController =
        instance ?: synchronized(this) {
            instance ?: GuestRuntime(context.applicationContext).also { instance = it }
        }

    internal fun impl(context: Context): GuestRuntime = get(context) as GuestRuntime
}

interface RuntimeController {
    val state: StateFlow<RuntimeState>
    val isSupportedAbi: Boolean

    /** Bootstrap if needed, then run the engine under [RuntimeService]. */
    fun start()
    fun stop()

    /**
     * Restart the engine process (bootstrap is kept), e.g. so it adopts the
     * account it just signed in to or out of: its workspace is fixed per run.
     * Starts it when it isn't running.
     */
    fun restart()

    /**
     * A development edge the engine joins instead of Zeron's (`zeron
     * local-edge` on another machine), or null for the default: the signed-in
     * account, else local-only. Takes effect on the next (re)start.
     */
    var customServer: CustomServer?

    /** Wipe the guest (keeps nothing, secrets included). */
    suspend fun reset()
    fun logTail(lines: Int = 200): String

    /** One-off `sh -lc` in the guest with the engine's environment. */
    suspend fun exec(
        command: String,
        asRoot: Boolean = false,
        timeoutMs: Long = 600_000,
    ): ExecResult

    /**
     * Host directory holding the guest's `/` (app-private). Guest paths the
     * engine reports map onto it, except `/tmp`, which is [guestTmpDir].
     */
    val guestRootDir: File

    /** Host directory bound at the guest's `/tmp` (and `/dev/shm`). */
    val guestTmpDir: File

}

sealed interface RuntimeState {
    data object NotInstalled : RuntimeState
    data class Bootstrapping(val step: String, val progress: Float?) : RuntimeState
    data object Starting : RuntimeState
    /** The engine answers on its IPC port; ask it for its edge and identity. */
    data class Running(
        val ipcPort: Int,
        val ipcToken: String,
        val deviceName: String,
    ) : RuntimeState {
        val ipcUrl: String get() = "ws://127.0.0.1:$ipcPort"
    }
    data object Stopped : RuntimeState
    data class Failed(val reason: String, val logTail: String) : RuntimeState
}

data class ExecResult(val exitCode: Int, val output: String)

/**
 * A development edge (`zeron local-edge`): its URL (e.g.
 * `http://10.0.2.2:27700`) and shared secret. The engine joins it in
 * Development scope (`ZERON_EDGE_URL`/`ZERON_EDGE_TOKEN`, docs/android.md
 * § Custom server).
 */
data class CustomServer(val edgeUrl: String, val token: String) {
    companion object {
        /** Why [url] / [token] can't be used, or null when they can (the edge's own checks). */
        fun problem(url: String, token: String): String? {
            val u = url.trim()
            val t = token.trim()
            return when {
                !(u.startsWith("http://") || u.startsWith("https://")) -> "The server URL must start with http:// or https://"
                runCatching { java.net.URI(u).host }.getOrNull().isNullOrEmpty() -> "That URL has no host."
                t.length < 16 -> "The token must be at least 16 characters."
                !t.all { it.isLetterOrDigit() && it.code < 128 || it in "._~-" } ->
                    "The token may only use letters, digits and . _ ~ -"
                else -> null
            }
        }

        /** Trimmed, without a trailing slash. */
        fun of(url: String, token: String) = CustomServer(url.trim().trimEnd('/'), token.trim())
    }
}
