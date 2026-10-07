package sh.zeron.android.core

/**
 * Why a saved computer can't be reached, in terms of the phone's own
 * network: which addresses the dial order has, whether the phone is on the
 * computer's Wi-Fi, and whether Tailscale is running (and installed) here.
 *
 * Two moments use it. [preflight] runs before / while dialling: when nothing
 * in the dial order can work on this network (off the home Wi-Fi with
 * Tailscale off, say) the chip turns red at once with the reason instead of
 * spinning through SSH timeouts. [diagnose] runs once the core reports a
 * failure and adds what the per-address errors tell (the computer may be
 * asleep, SSH sign-in failed, Zeron isn't running there…).
 */
object ConnectionDiagnosis {
    enum class Kind {
        /** No Wi-Fi, no mobile data. */
        NO_NETWORK,
        /** The route is Tailscale and Tailscale isn't running on the phone. */
        TAILSCALE_OFF,
        /** Not on the computer's Wi-Fi, and Tailscale is off. */
        AWAY_TAILSCALE_OFF,
        /** Tailscale is needed but isn't installed on the phone. */
        TAILSCALE_MISSING,
        /** Not on the computer's Wi-Fi, and it has only LAN addresses (or only a LAN one is picked). */
        NOT_SAME_WIFI,
        /** Tailscale is on, but the computer didn't answer through it. */
        PC_UNREACHABLE,
        /** On the computer's Wi-Fi, but it didn't answer (and there's no Tailscale fallback). */
        LAN_UNREACHABLE,
        /** Nothing network-specific: the classified engine / SSH error says it (auth, Zeron not running…). */
        ISSUE,
    }

    enum class TailscaleState { ON, OFF, MAYBE }

    data class Input(
        /** Every address of the computer. */
        val addresses: List<Endpoint>,
        /** What the dial order holds: every address with auto-select on, else the picked one. */
        val dialled: List<Endpoint>,
        val net: NetworkSnapshot,
        val tailscaleInstalled: Boolean,
        val autoRoute: Boolean,
    )

    data class Result(
        val kind: Kind,
        /** The classified core error ([Kind.ISSUE]), or the closest one for the details fold. */
        val issue: ConnectionIssue.Kind,
        /** Found before the dial gave up: the link retries by itself once the network changes. */
        val early: Boolean = false,
        /** Tailscale is off because another VPN holds the phone's single VPN slot. */
        val otherVpn: Boolean = false,
        /** The computer's LAN address was tried on its Wi-Fi first, and failed. */
        val lanFailed: Boolean = false,
        /** Auto-select is off and a LAN address is picked although the computer has others. */
        val pickedLan: Boolean = false,
    ) {
        /** Show "打开 Tailscale" ("Open Tailscale"). */
        val opensTailscale: Boolean get() = kind == Kind.TAILSCALE_OFF || kind == Kind.AWAY_TAILSCALE_OFF
        /** Show "安装 Tailscale" ("Install Tailscale"). */
        val installsTailscale: Boolean get() = kind == Kind.TAILSCALE_MISSING
    }

    fun tailscale(net: NetworkSnapshot): TailscaleState = when (net.vpn) {
        NetworkSnapshot.Vpn.TAILSCALE -> TailscaleState.ON
        NetworkSnapshot.Vpn.NONE, NetworkSnapshot.Vpn.OTHER -> TailscaleState.OFF
        NetworkSnapshot.Vpn.UNKNOWN -> TailscaleState.MAYBE
    }

    /**
     * Before the dial: null when some address in the dial order can work on
     * this network, else why none can.
     */
    fun preflight(input: Input): Result? {
        if (input.net.transport == NetworkSnapshot.Transport.NONE) {
            return Result(Kind.NO_NETWORK, ConnectionIssue.Kind.UNREACHABLE, early = true)
        }
        if (input.dialled.isEmpty()) return null
        if (input.dialled.any { RoutePlanner.tier(it, input.net) < RoutePlanner.UNREACHABLE }) return null
        return unreachableHere(input).copy(early = true)
    }

    /**
     * After a failure: [lastError] is the core's most telling error,
     * [endpointErrors] each tried address's last error by [Endpoint.key].
     * Sign-in and engine problems come first: SSH got that far, so the
     * network is fine.
     */
    fun diagnose(input: Input, lastError: String?, endpointErrors: Map<String, String?> = emptyMap()): Result {
        val issue = ConnectionIssue.classify(lastError)
        if (issue.needsUser || issue == ConnectionIssue.Kind.ENGINE || issue == ConnectionIssue.Kind.SYNC) {
            return Result(Kind.ISSUE, issue)
        }
        preflight(input)?.let { return it.copy(early = false, issue = issue) }
        if (issue != ConnectionIssue.Kind.TIMEOUT && issue != ConnectionIssue.Kind.UNREACHABLE &&
            issue != ConnectionIssue.Kind.UNKNOWN && issue != ConnectionIssue.Kind.LOST
        ) {
            // Refused (SSH off), DNS…: the classified error is already the plain answer.
            return Result(Kind.ISSUE, issue)
        }
        val ts = input.dialled.filter { it.kind == EndpointKind.TAILSCALE }
        val lanHere = input.dialled.filter { it.kind == EndpointKind.LAN && RoutePlanner.tier(it, input.net) < RoutePlanner.UNREACHABLE }
        val lanFailed = lanHere.isNotEmpty() && lanHere.all { endpointErrors[it.key] != null || endpointErrors.isEmpty() }
        val state = tailscale(input.net)
        return when {
            ts.isNotEmpty() && state == TailscaleState.OFF -> tailscaleOff(input, lanFailed)
            ts.isNotEmpty() -> Result(Kind.PC_UNREACHABLE, issue, lanFailed = lanFailed)
            lanHere.isNotEmpty() -> Result(Kind.LAN_UNREACHABLE, issue, lanFailed = true)
            else -> Result(Kind.ISSUE, issue)
        }
    }

    /** Nothing in the dial order can work on this network. */
    private fun unreachableHere(input: Input): Result {
        val ts = input.dialled.any { it.kind == EndpointKind.TAILSCALE }
        if (ts) {
            // Here every LAN address in the order is off this network.
            val away = input.dialled.any { it.kind == EndpointKind.LAN }
            return tailscaleOff(input, lanFailed = false, away = away)
        }
        val pickedLan = !input.autoRoute && input.addresses.any { it.kind != EndpointKind.LAN }
        return Result(Kind.NOT_SAME_WIFI, ConnectionIssue.Kind.UNREACHABLE, pickedLan = pickedLan)
    }

    private fun tailscaleOff(input: Input, lanFailed: Boolean, away: Boolean = false): Result {
        val otherVpn = input.net.vpn == NetworkSnapshot.Vpn.OTHER
        val kind = when {
            !input.tailscaleInstalled -> Kind.TAILSCALE_MISSING
            away -> Kind.AWAY_TAILSCALE_OFF
            else -> Kind.TAILSCALE_OFF
        }
        return Result(kind, ConnectionIssue.Kind.UNREACHABLE, otherVpn = otherVpn, lanFailed = lanFailed)
    }
}
