package sh.zeron.android.core

import android.net.Network
import android.net.NetworkCapabilities

/**
 * What a direct connection's path depends on: which network, over what, and whether it's metered.
 * Signal strength and bandwidth change all the time and leave this alone, so a re-delivered
 * network doesn't tear down a healthy direct link.
 */
fun networkShape(network: Network, capabilities: NetworkCapabilities): String {
    val transports = (0..TRANSPORT_MAX).filter(capabilities::hasTransport).joinToString(",")
    val metered = !capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED)
    return "$network/$transports" + if (metered) "!" else ""
}

/** Past the highest transport the platform defines today. An unknown value just reads as absent. */
private const val TRANSPORT_MAX = 15

/**
 * Turns what the OS says about the default network into the core's two calls, each only when it
 * means something. Main thread only.
 *
 * Android says less than iOS's path monitor does, which is why this exists:
 * - With no network at launch it says nothing at all, so [seed] reads the state first.
 * - Moving between networks needn't pass through "lost".
 * - In Doze, or with background data restricted, the network stays up and only this app is cut off.
 *   Dials can't succeed then, so that counts as no path: they park instead of retrying.
 */
class NetworkPathTracker(
    private val setOnline: (Boolean) -> Unit,
    private val pathChanged: () -> Unit,
) {
    private var shape: String? = null
    private var blocked = false

    /** Whether this app can use a network right now. */
    val online: Boolean get() = shape != null && !blocked

    /** The state before the first callback. Reports nothing: a new client is told with [online]. */
    fun seed(shape: String?) {
        this.shape = shape
    }

    /** The default network's capabilities, or null when there's no default network any more. */
    fun update(next: String?) {
        val was = online
        val last = shape
        shape = next
        if (next == null) blocked = false
        report(was, moved = last != null && next != null && last != next)
    }

    fun setBlocked(blocked: Boolean) {
        val was = online
        this.blocked = blocked
        report(was, moved = false)
    }

    private fun report(was: Boolean, moved: Boolean) {
        val now = online
        // Coming back online already counts as a path change in the core.
        if (now != was) setOnline(now) else if (now && moved) pathChanged()
    }
}
