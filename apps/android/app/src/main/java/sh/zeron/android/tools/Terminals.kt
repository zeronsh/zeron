package sh.zeron.android.tools

import android.os.Handler
import android.os.Looper
import androidx.compose.runtime.MutableState
import androidx.compose.runtime.RememberObserver
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshots.SnapshotStateList
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import org.json.JSONObject
import sh.zeron.android.core.AppModel
import uniffi.zeron_core.CoreClient
import uniffi.zeron_core.TerminalFrame
import uniffi.zeron_core.TerminalListener
import uniffi.zeron_core.TerminalPalette
import uniffi.zeron_core.TerminalScreen

/** One open engine terminal, as a tab. */
data class TerminalTab(val id: String, val shell: String, val cwd: String?, val title: String? = null) {
    val label: String get() = title?.takeIf { it.isNotBlank() } ?: shell
}

/**
 * The terminals open per workspace, in memory for the app's lifetime. The
 * engine keeps each shell running while no screen shows it (and replays its
 * scrollback on reattach), so tabs survive navigating away and back.
 */
object Terminals {
    /** Opens and kills outlive the screen that asked for them. */
    val scope: CoroutineScope = MainScope()
    private val tabs = HashMap<String, SnapshotStateList<TerminalTab>>()
    private val selection = HashMap<String, MutableState<String?>>()

    fun key(ref: WorkspaceRef): String = "${ref.deviceId}|${ref.chatId ?: ref.spaceId}"

    fun tabs(key: String): SnapshotStateList<TerminalTab> = tabs.getOrPut(key) { mutableStateListOf() }

    /** The selected tab's terminal id. */
    fun selection(key: String): MutableState<String?> = selection.getOrPut(key) { mutableStateOf(null) }

    /** `OpenTerminal` in the workspace's folder (the engine picks the chat's cwd). */
    suspend fun open(model: AppModel, ref: WorkspaceRef, cols: Int, rows: Int): TerminalTab {
        val params = JSONObject()
            .put("chatId", ref.chatId ?: "space-canvas:${ref.spaceId}")
            .put("cols", cols)
            .put("rows", rows)
        val reply = model.hostCall(ref.deviceId, OPEN, params) as JSONObject
        return TerminalTab(
            id = reply.getString("id"),
            shell = reply.optString("shell").ifEmpty { "shell" },
            cwd = reply.optString("cwd").ifEmpty { null },
        )
    }

    fun add(key: String, tab: TerminalTab) {
        tabs(key) += tab
        selection(key).value = tab.id
    }

    /** Put [tab] where [oldId] was (Restart keeps the tab's place). */
    fun replace(key: String, oldId: String, tab: TerminalTab) {
        val list = tabs(key)
        val i = list.indexOfFirst { it.id == oldId }
        if (i >= 0) list[i] = tab else list += tab
        selection(key).value = tab.id
    }

    fun rename(key: String, id: String, title: String?) {
        val list = tabs(key)
        val i = list.indexOfFirst { it.id == id }
        if (i >= 0 && list[i].title != title) list[i] = list[i].copy(title = title)
    }

    /** Drop the tab, select a neighbour, and end its shell (`CloseTerminal`). */
    suspend fun kill(model: AppModel, ref: WorkspaceRef, id: String) {
        val key = key(ref)
        val list = tabs(key)
        val i = list.indexOfFirst { it.id == id }
        if (i >= 0) list.removeAt(i)
        val selected = selection(key)
        if (selected.value == id) selected.value = list.getOrNull((i - 1).coerceAtLeast(0))?.id
        forget(model, ref.deviceId, id)
    }

    /** `CloseTerminal`, ignoring a terminal that's already gone. */
    suspend fun forget(model: AppModel, deviceId: String, id: String) {
        runCatching { model.hostCall(deviceId, CLOSE, JSONObject().put("terminalId", id)) }
    }

    const val OPEN = "OpenTerminal"
    const val CLOSE = "CloseTerminal"
    const val RESIZE = "ResizeTerminal"
}

/**
 * A view attached to one engine terminal: owns the Rust [TerminalScreen] and
 * mirrors what the chrome needs (title, exit, selection) as Compose state.
 * Forgetting it detaches (`destroy`) and leaves the shell running.
 */
class TerminalSession(
    private val model: AppModel,
    client: CoreClient,
    private val deviceId: String,
    val id: String,
    cols: Int,
    rows: Int,
    palette: TerminalPalette,
) : RememberObserver {
    private val main = Handler(Looper.getMainLooper())
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)
    private var detached = false
    private var sizeSynced = false

    var title by mutableStateOf<String?>(null)
        private set
    var connecting by mutableStateOf(true)
        private set
    var exitCode by mutableStateOf<Int?>(null)
        private set
    var hasSelection by mutableStateOf(false)
        private set
    var scrolledBack by mutableStateOf(false)
        private set

    /** The last pulled grid (painted by [TerminalView]). */
    var frame: TerminalFrame? = null
        private set

    /** Repaint hook, set by the view showing this session. */
    var onFrame: (() -> Unit)? = null

    private val listener = object : TerminalListener {
        override fun onFrame() {
            main.post { pull() }
        }

        override fun onExit(code: Int) {
            main.post {
                if (!detached) exitCode = code
                pull()
            }
        }
    }

    val screen: TerminalScreen = client.terminalScreen(deviceId, id, cols.toUShort(), rows.toUShort(), palette, listener)

    init {
        pull()
    }

    /** Take the pending frame (re-arms the Rust side's coalesced notify). */
    private fun pull() {
        if (detached) return
        val f = runCatching { screen.frame() }.getOrNull() ?: return
        frame = f
        title = f.title?.takeIf { it.isNotBlank() }
        connecting = f.connecting
        f.exitCode?.let { exitCode = it }
        hasSelection = f.hasSelection
        scrolledBack = f.displayOffset > 0u
        onFrame?.invoke()
    }

    /** Resize the grid; the first call also tells the engine outright, since
     *  a reattach at an unchanged size wouldn't send a resize at all. */
    fun resize(cols: Int, rows: Int) {
        if (detached) return
        screen.resize(cols.toUShort(), rows.toUShort())
        if (!sizeSynced) {
            sizeSynced = true
            scope.launch {
                runCatching {
                    model.hostCall(deviceId, Terminals.RESIZE, JSONObject().put("terminalId", id).put("cols", cols).put("rows", rows))
                }
            }
        }
    }

    fun setPalette(palette: TerminalPalette) {
        if (!detached) screen.setPalette(palette)
    }

    val isAttached: Boolean get() = !detached

    /** Stop following; the engine keeps the shell. */
    fun detach() {
        if (detached) return
        detached = true
        onFrame = null
        scope.cancel()
        main.removeCallbacksAndMessages(null)
        screen.destroy()
    }

    override fun onRemembered() {}
    override fun onForgotten() = detach()
    override fun onAbandoned() = detach()
}
