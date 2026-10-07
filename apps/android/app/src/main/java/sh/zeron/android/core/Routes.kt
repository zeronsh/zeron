package sh.zeron.android.core

import java.util.Locale

/**
 * How the phone reaches a computer that has several addresses (its LAN IP
 * at home, its Tailscale IP from anywhere, maybe a DNS name). Pure logic,
 * unit-tested: address kinds, the dial order for the network the phone is
 * on, when a network change should move the link, and grouping saved
 * computers that are really the same machine.
 */

/** One address of a computer. Same SSH server, credentials and host key as its others. */
data class Endpoint(val host: String, val port: Int = 22) {
    val kind: EndpointKind get() = EndpointKind.of(host)

    /** Identity for de-duplication and "last used here" memory. */
    val key: String get() = "${host.trim().lowercase(Locale.ROOT)}:$port"

    fun display(): String = if (port == 22) host.trim() else "${host.trim()}:$port"

    fun toJson(): org.json.JSONObject = org.json.JSONObject().put("host", host).put("port", port)

    companion object {
        fun fromJson(o: org.json.JSONObject) = Endpoint(o.optString("host"), o.optInt("port", 22))

        /** "192.168.1.102", "100.1.2.3:2222", "[fd7a::1]:22" -> Endpoint (port defaults to [defaultPort]). */
        fun parse(text: String, defaultPort: Int = 22): Endpoint? {
            val t = text.trim()
            if (t.isEmpty()) return null
            if (t.startsWith("[")) {
                val end = t.indexOf(']')
                if (end < 0) return null
                val port = t.substring(end + 1).removePrefix(":").toIntOrNull() ?: defaultPort
                return Endpoint(t.substring(1, end), port)
            }
            val colons = t.count { it == ':' }
            if (colons == 1) {
                val port = t.substringAfter(':').toIntOrNull() ?: return null
                return Endpoint(t.substringBefore(':'), port)
            }
            return Endpoint(t, defaultPort)
        }
    }
}

enum class EndpointKind(val wire: String) {
    /** Private address or mDNS name: reachable on the same Wi-Fi / LAN. */
    LAN("lan"),
    /** Tailscale CGNAT range, its IPv6 ULA, or a MagicDNS *.ts.net name. */
    TAILSCALE("tailscale"),
    OTHER("other");

    companion object {
        fun fromWire(wire: String): EndpointKind = entries.firstOrNull { it.wire == wire } ?: OTHER

        fun of(rawHost: String): EndpointKind {
            val host = rawHost.trim().trim('[', ']').lowercase(Locale.ROOT).trimEnd('.')
            ipv4(host)?.let { ip ->
                return when {
                    Subnet(ipv4("100.64.0.0")!!, 10).contains(ip) -> TAILSCALE
                    PRIVATE_V4.any { it.contains(ip) } -> LAN
                    else -> OTHER
                }
            }
            if (host.contains(':')) {
                return when {
                    host.startsWith("fd7a:115c:a1e0:") -> TAILSCALE
                    // Link-local fe80::/10 and unique-local fc00::/7.
                    host.startsWith("fe8") || host.startsWith("fe9") || host.startsWith("fea") || host.startsWith("feb") -> LAN
                    host.startsWith("fc") || host.startsWith("fd") -> LAN
                    else -> OTHER
                }
            }
            return when {
                host.endsWith(".ts.net") -> TAILSCALE
                LAN_SUFFIXES.any { host.endsWith(it) } -> LAN
                else -> OTHER
            }
        }

        private val PRIVATE_V4 = listOf(
            Subnet(ipv4("10.0.0.0")!!, 8),
            Subnet(ipv4("172.16.0.0")!!, 12),
            Subnet(ipv4("192.168.0.0")!!, 16),
            Subnet(ipv4("169.254.0.0")!!, 16),
        )
        private val LAN_SUFFIXES = listOf(".local", ".lan", ".home", ".home.arpa", ".localdomain", ".internal")
    }
}

/** Dotted IPv4 -> Int, or null. */
fun ipv4(text: String): Int? {
    val parts = text.trim().split('.')
    if (parts.size != 4) return null
    var v = 0
    for (p in parts) {
        if (p.isEmpty() || p.length > 3 || !p.all { it.isDigit() }) return null
        val n = p.toInt()
        if (n > 255) return null
        v = (v shl 8) or n
    }
    return v
}

/** An IPv4 network, e.g. the home Wi-Fi's 192.168.1.0/24. */
data class Subnet(val base: Int, val prefix: Int) {
    private val mask: Int get() = if (prefix == 0) 0 else -1 shl (32 - prefix)

    fun contains(ip: Int): Boolean = (ip and mask) == (base and mask)

    fun contains(host: String): Boolean = ipv4(host.trim())?.let { contains(it) } ?: false

    override fun toString(): String {
        val b = base and mask
        return "${(b ushr 24) and 255}.${(b ushr 16) and 255}.${(b ushr 8) and 255}.${b and 255}/$prefix"
    }

    companion object {
        fun of(address: String, prefix: Int): Subnet? = ipv4(address)?.let { Subnet(it, prefix) }
    }
}

/** What the phone is connected through right now. */
data class NetworkSnapshot(
    val transport: Transport,
    /** IPv4 networks of the Wi-Fi / Ethernet link underneath any VPN. */
    val localSubnets: List<Subnet> = emptyList(),
    val vpn: Vpn = Vpn.NONE,
) {
    enum class Transport { WIFI, ETHERNET, CELLULAR, OTHER, NONE }

    /** Tailscale and a proxy app (Clash…) can't both hold Android's VPN slot. */
    enum class Vpn { NONE, TAILSCALE, OTHER, UNKNOWN }

    val onLocalNetwork: Boolean get() = transport == Transport.WIFI || transport == Transport.ETHERNET

    /**
     * Which network this is, for "the address that worked here last time":
     * the Wi-Fi's subnet stands in for its name (the SSID needs the location
     * permission), mobile data is one network.
     */
    val key: String
        get() = when (transport) {
            Transport.WIFI, Transport.ETHERNET ->
                transport.name.lowercase(Locale.ROOT) + ":" + (localSubnets.firstOrNull()?.toString() ?: "?")
            else -> transport.name.lowercase(Locale.ROOT)
        }

    companion object {
        val UNKNOWN = NetworkSnapshot(Transport.OTHER, vpn = Vpn.UNKNOWN)
    }
}

/** Dial order for a computer's addresses on the current network. */
object RoutePlanner {
    /** A LAN address answers in milliseconds when it's there at all. */
    const val LAN_HEAD_START_MS = 1_500
    /** Tailscale may relay through DERP on a first connect; give it longer. */
    const val HEAD_START_MS = 4_000

    /**
     * How long reaching an address (TCP + SSH handshake) may take before it
     * counts as failed. A LAN host answers in milliseconds when it's there;
     * Tailscale may set up a DERP relay first. 0 = the core's 20 s.
     */
    const val LAN_TIMEOUT_MS = 4_000
    const val TAILSCALE_TIMEOUT_MS = 12_000

    fun connectTimeoutMs(kind: EndpointKind): Int = when (kind) {
        EndpointKind.LAN -> LAN_TIMEOUT_MS
        EndpointKind.TAILSCALE -> TAILSCALE_TIMEOUT_MS
        EndpointKind.OTHER -> 0
    }

    /** Tier this high or more: can't work on this network (LAN off Wi-Fi, Tailscale with its VPN off). */
    const val UNREACHABLE = 4

    data class Planned(val endpoint: Endpoint, val kind: EndpointKind, val tier: Int, val headStartMs: Int)

    /** Lower = try first. */
    fun tier(endpoint: Endpoint, net: NetworkSnapshot): Int = when (endpoint.kind) {
        EndpointKind.LAN -> when {
            !net.onLocalNetwork -> 5
            net.localSubnets.any { it.contains(endpoint.host) } -> 0
            ipv4(endpoint.host.trim()) == null -> 1 // .local name / IPv6: can't tell, likely here
            net.localSubnets.isEmpty() -> 1
            else -> 2 // a private address, but not this Wi-Fi's
        }
        EndpointKind.TAILSCALE -> when (net.vpn) {
            NetworkSnapshot.Vpn.TAILSCALE, NetworkSnapshot.Vpn.UNKNOWN -> 1
            NetworkSnapshot.Vpn.NONE, NetworkSnapshot.Vpn.OTHER -> UNREACHABLE
        }
        EndpointKind.OTHER -> 2
    }

    /**
     * [addresses] ordered for [net]: the address that worked on this network
     * last time ([remembered], an [Endpoint.key]) first if it can still work,
     * then by tier, keeping the user's order within a tier. Nothing is
     * dropped: an "unreachable" address is still tried last.
     */
    /**
     * Auto-select route off: only the address the user picked ([pinned], an
     * [Endpoint.key]), or the first one if none / it's gone. No fallback.
     */
    fun manual(addresses: List<Endpoint>, pinned: String?): List<Planned> {
        val e = addresses.firstOrNull { it.key == pinned } ?: addresses.firstOrNull() ?: return emptyList()
        return listOf(Planned(e, e.kind, 0, 0))
    }

    fun plan(addresses: List<Endpoint>, net: NetworkSnapshot, remembered: String? = null): List<Planned> {
        val ranked = addresses.distinctBy { it.key }.map { e ->
            val t = tier(e, net)
            e to if (e.key == remembered && t <= 2) -1 else t
        }.sortedBy { it.second }
        return ranked.mapIndexed { i, (e, t) ->
            val last = i == ranked.lastIndex
            val head = when {
                last -> 0
                e.kind == EndpointKind.LAN -> LAN_HEAD_START_MS
                else -> HEAD_START_MS
            }
            Planned(e, e.kind, t, head)
        }
    }
}

/** Whether a network change should drop a working link to move to a better address. */
object RouteSwitch {
    /** Don't bounce between addresses more often than this for a mere upgrade. */
    const val MIN_UPGRADE_GAP_MS = 30_000L

    enum class Link { LIVE, CONNECTING, FAILED }

    enum class Action { STAY, RECONNECT }

    /**
     * [activeKey]: the address the link is on (null when none). A failed or
     * still-dialling link redials at once with the new order; a live link
     * moves at once when its address can't work any more (left the home
     * Wi-Fi, Tailscale switched off) and for a better one (home again) only
     * if it hasn't moved in the last [MIN_UPGRADE_GAP_MS].
     */
    fun decide(plan: List<RoutePlanner.Planned>, activeKey: String?, link: Link, sinceLastMoveMs: Long): Action {
        if (plan.isEmpty()) return Action.STAY
        if (link != Link.LIVE) return Action.RECONNECT
        val active = plan.firstOrNull { it.endpoint.key == activeKey } ?: return Action.RECONNECT
        if (active.tier >= RoutePlanner.UNREACHABLE) return Action.RECONNECT
        val best = plan.first()
        if (best.endpoint.key != active.endpoint.key && best.tier < active.tier && sinceLastMoveMs >= MIN_UPGRADE_GAP_MS) return Action.RECONNECT
        return Action.STAY
    }
}

/**
 * Saved computers that are the same machine, e.g. one saved with its LAN IP
 * and one with its Tailscale IP. Same user, engine port and sign-in method,
 * no conflicting host keys, and either the same pinned host key or the same
 * name. The first of a group keeps its id; the others' addresses join it.
 */
object MachineGroups {
    data class Result(val machines: List<Machine>, val mergedInto: Map<String, String>)

    fun sameComputer(a: Machine, b: Machine): Boolean {
        if (!a.user.trim().equals(b.user.trim(), ignoreCase = true)) return false
        if (a.enginePort != b.enginePort || a.auth != b.auth) return false
        val ka = a.hostKey?.takeIf { it.isNotBlank() }
        val kb = b.hostKey?.takeIf { it.isNotBlank() }
        if (ka != null && kb != null) return ka == kb
        return a.name.isNotBlank() && a.name.trim().equals(b.name.trim(), ignoreCase = true)
    }

    fun merge(machines: List<Machine>): Result {
        val out = mutableListOf<Machine>()
        val into = mutableMapOf<String, String>()
        for (m in machines) {
            val i = out.indexOfFirst { sameComputer(it, m) }
            if (i < 0) {
                out += m
            } else {
                out[i] = combine(out[i], m)
                into[m.id] = out[i].id
            }
        }
        return Result(out, into)
    }

    /** [into] gains [other]'s addresses (and its name / host key where it has none). */
    fun combine(into: Machine, other: Machine): Machine = into.withAddresses((into.addresses() + other.addresses()).distinctBy { it.key })
        .copy(
            name = into.name.ifBlank { other.name },
            hostKey = into.hostKey?.takeIf { it.isNotBlank() } ?: other.hostKey,
        )
}
