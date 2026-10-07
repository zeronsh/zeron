package sh.zeron.android.core

import uniffi.zeron_core.ConnectivityState
import uniffi.zeron_core.DirectPhase

/**
 * The home title bar's connection chip: which workspace is active and a
 * traffic-light dot (green connected, yellow pulsing while connecting or
 * reconnecting, red failed / offline; gray for the offline demo).
 */
object ConnectionState {
    enum class Dot { CONNECTED, CONNECTING, FAILED, NEUTRAL }

    enum class Workspace { DEMO, DIRECT, CLOUD }

    data class View(
        val title: String,
        val workspace: Workspace,
        val dot: Dot,
        /** Raw failure text from the link (direct mode), for the failure sheet. */
        val error: String? = null,
        /** When the core retries next (epoch ms), if it will. */
        val retryAtMs: Long? = null,
        /** The workspace id: a machine id, "cloud" or "demo". */
        val id: String = "",
        /** Which of the computer's addresses the link runs over (LAN, Tailscale…), when connected. */
        val route: EndpointKind? = null,
        /** Why it's down in terms of the phone's network (Tailscale off…), when known. */
        val diagnosis: ConnectionDiagnosis.Result? = null,
    )

    fun dot(workspace: Workspace, loading: Boolean, direct: DirectPhase?, cloud: ConnectivityState?): Dot = when {
        workspace == Workspace.DEMO -> Dot.NEUTRAL
        loading -> Dot.CONNECTING
        workspace == Workspace.DIRECT -> when (direct) {
            DirectPhase.LIVE -> Dot.CONNECTED
            DirectPhase.FAILED -> Dot.FAILED
            // Not started yet, dialing, or loading the first snapshot.
            null, DirectPhase.CONNECTING, DirectPhase.SYNCING -> Dot.CONNECTING
        }
        else -> when (cloud) {
            ConnectivityState.CONNECTED -> Dot.CONNECTED
            ConnectivityState.OFFLINE -> Dot.FAILED
            null, ConnectivityState.DISABLED, ConnectivityState.RECONNECTING -> Dot.CONNECTING
        }
    }

    /**
     * The failure sheet pops once per failure episode: the first FAILED
     * since the link was last connected (or since switching workspace).
     * The core's automatic retries flip FAILED -> CONNECTING -> FAILED
     * without popping it again; a red dot tap re-opens it by hand.
     */
    class Episodes {
        private var shown = false

        /** True when the sheet should pop for this dot. */
        fun update(dot: Dot): Boolean = when (dot) {
            Dot.CONNECTED, Dot.NEUTRAL -> { shown = false; false }
            Dot.FAILED -> if (shown) false else { shown = true; true }
            Dot.CONNECTING -> false
        }

        fun reset() { shown = false }
    }
}
