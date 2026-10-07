package sh.zeron.android.core

import android.content.Context
import android.net.ConnectivityManager
import android.net.LinkProperties
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import java.net.Inet4Address
import java.util.concurrent.ConcurrentHashMap

/**
 * Keeps a [NetworkSnapshot] of every network the app can see (Wi-Fi,
 * mobile data, and a VPN such as Tailscale or Clash), so the dial order can
 * follow the phone from home Wi-Fi to mobile data and back. Only needs
 * ACCESS_NETWORK_STATE: the Wi-Fi is told apart by its subnet, not its
 * name (that would need the location permission).
 */
class NetworkWatcher(context: Context, private val onChange: (NetworkSnapshot) -> Unit) {
    private val cm = context.getSystemService(ConnectivityManager::class.java)
    private val caps = ConcurrentHashMap<Network, NetworkCapabilities>()
    private val links = ConcurrentHashMap<Network, LinkProperties>()
    @Volatile var snapshot: NetworkSnapshot = cm?.let { current(it) } ?: NetworkSnapshot.UNKNOWN
        private set

    private val callback = object : ConnectivityManager.NetworkCallback() {
        override fun onCapabilitiesChanged(network: Network, capabilities: NetworkCapabilities) {
            caps[network] = capabilities
            changed()
        }

        override fun onLinkPropertiesChanged(network: Network, linkProperties: LinkProperties) {
            links[network] = linkProperties
            changed()
        }

        override fun onLost(network: Network) {
            caps.remove(network)
            links.remove(network)
            changed()
        }
    }

    fun start() {
        val request = NetworkRequest.Builder()
            .addCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
            .removeCapability(NetworkCapabilities.NET_CAPABILITY_NOT_VPN)
            .build()
        runCatching { cm?.registerNetworkCallback(request, callback) }
    }

    fun stop() {
        runCatching { cm?.unregisterNetworkCallback(callback) }
    }

    private fun changed() {
        val next = build(caps.keys.mapNotNull { n -> caps[n]?.let { View(it, links[n]) } })
        if (next != snapshot) {
            snapshot = next
            onChange(next)
        }
    }

    /** One network as the builder needs it. */
    private class View(val caps: NetworkCapabilities, val link: LinkProperties?)

    companion object {
        /** A one-off reading (scheduled sends open the core without the app's watcher). */
        @Suppress("DEPRECATION")
        fun current(cm: ConnectivityManager): NetworkSnapshot = runCatching {
            build(cm.allNetworks.mapNotNull { n -> cm.getNetworkCapabilities(n)?.let { View(it, cm.getLinkProperties(n)) } })
        }.getOrDefault(NetworkSnapshot.UNKNOWN)

        fun current(context: Context): NetworkSnapshot =
            context.getSystemService(ConnectivityManager::class.java)?.let { current(it) } ?: NetworkSnapshot.UNKNOWN

        private fun build(networks: List<View>): NetworkSnapshot {
            val (vpns, plain) = networks.partition { it.caps.hasTransport(NetworkCapabilities.TRANSPORT_VPN) }
            fun has(t: Int) = plain.firstOrNull { it.caps.hasTransport(t) }
            val wifi = has(NetworkCapabilities.TRANSPORT_WIFI)
            val eth = has(NetworkCapabilities.TRANSPORT_ETHERNET)
            val cell = has(NetworkCapabilities.TRANSPORT_CELLULAR)
            val (transport, local) = when {
                wifi != null -> NetworkSnapshot.Transport.WIFI to wifi
                eth != null -> NetworkSnapshot.Transport.ETHERNET to eth
                cell != null -> NetworkSnapshot.Transport.CELLULAR to null
                plain.isNotEmpty() -> NetworkSnapshot.Transport.OTHER to null
                vpns.isNotEmpty() -> NetworkSnapshot.Transport.OTHER to null
                else -> NetworkSnapshot.Transport.NONE to null
            }
            val subnets = local?.link?.linkAddresses.orEmpty()
                .filter { it.address is Inet4Address }
                .mapNotNull { Subnet.of(it.address.hostAddress.orEmpty(), it.prefixLength) }
            val vpn = when {
                vpns.isEmpty() -> NetworkSnapshot.Vpn.NONE
                else -> {
                    val addresses = vpns.flatMap { it.link?.linkAddresses.orEmpty() }.map { it.address.hostAddress.orEmpty() }
                    when {
                        // The VPN's addresses aren't visible: a tailnet address on
                        // an interface (Tailscale's tun) still tells.
                        addresses.isEmpty() -> if (Tailscale.hasTailnetInterface()) NetworkSnapshot.Vpn.TAILSCALE else NetworkSnapshot.Vpn.UNKNOWN
                        addresses.any { EndpointKind.of(it.substringBefore('%')) == EndpointKind.TAILSCALE } -> NetworkSnapshot.Vpn.TAILSCALE
                        else -> NetworkSnapshot.Vpn.OTHER
                    }
                }
            }
            return NetworkSnapshot(transport, subnets, vpn)
        }
    }
}
