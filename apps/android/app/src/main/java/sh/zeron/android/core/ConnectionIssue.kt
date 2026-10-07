package sh.zeron.android.core

/**
 * A direct-link failure in words a person can act on. The engine / SSH layer
 * (crates/client/src/direct) reports English strings such as
 * "timed out reaching 192.168.1.20:22" or "the machine rejected this phone's
 * key for user …"; [classify] maps them onto a small set of kinds, each with
 * a localized title and hint in the failure sheet. The raw text is kept for
 * the collapsible "details" part.
 */
object ConnectionIssue {
    enum class Kind {
        TIMEOUT,
        REFUSED,
        UNREACHABLE,
        DNS,
        AUTH_KEY,
        AUTH_PASSWORD,
        AUTH,
        HOST_KEY_UNKNOWN,
        HOST_KEY_CHANGED,
        KEY,
        ENGINE,
        LOST,
        SYNC,
        OFFLINE,
        UNKNOWN,
        ;

        /** Retrying won't help until the user changes something (same idea as SshError::needs_user). */
        val needsUser: Boolean get() = this in setOf(AUTH_KEY, AUTH_PASSWORD, AUTH, HOST_KEY_UNKNOWN, HOST_KEY_CHANGED, KEY)
    }

    fun classify(raw: String?): Kind {
        val t = raw?.trim()?.lowercase().orEmpty()
        if (t.isEmpty()) return Kind.UNKNOWN
        return when {
            "host key changed" in t -> Kind.HOST_KEY_CHANGED
            "unknown host key" in t -> Kind.HOST_KEY_UNKNOWN
            "rejected this phone's key" in t || "publickey" in t -> Kind.AUTH_KEY
            "wrong password" in t -> Kind.AUTH_PASSWORD
            "private key" in t || "system rng" in t -> Kind.KEY
            // Engine side: SSH worked, Zeron didn't answer through the tunnel.
            "refused a tunnel" in t || "no zeron engine" in t || "engineinfo" in t ||
                "opening the tunnel" in t || "is zeron running" in t -> Kind.ENGINE
            "timed out reaching" in t || "timed out" in t || "timeout" in t -> Kind.TIMEOUT
            "refused the connection" in t || "connection refused" in t -> Kind.REFUSED
            "failed to lookup address" in t || "name or service not known" in t || "nodename nor servname" in t ||
                "no address associated" in t || "unknown host" in t || "name resolution" in t -> Kind.DNS
            "no route to host" in t || "network is unreachable" in t || "host is unreachable" in t ||
                "can't reach" in t -> Kind.UNREACHABLE
            "authentication" in t || "auth" in t -> Kind.AUTH
            "connection to the machine lost" in t || "ssh session closed" in t || "connection reset" in t ||
                "broken pipe" in t -> Kind.LOST
            "couldn't read" in t -> Kind.SYNC
            else -> Kind.UNKNOWN
        }
    }
}
