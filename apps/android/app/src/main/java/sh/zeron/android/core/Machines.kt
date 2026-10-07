package sh.zeron.android.core

import android.content.Context
import org.json.JSONArray
import org.json.JSONObject
import uniffi.zeron_core.SshAuth
import uniffi.zeron_core.SshTarget
import uniffi.zeron_core.sshGenerateKey
import java.util.UUID

/** A saved SSH machine. Secrets are NOT here: see [MachineStore]. */
data class Machine(
    val id: String = UUID.randomUUID().toString(),
    val name: String = "",
    val host: String = "",
    val port: Int = 22,
    val user: String = "",
    /** `phone` = this phone's generated key, `key` = an imported key, `password`. */
    val auth: String = AUTH_PHONE,
    val enginePort: Int = 27654,
    /** Pinned `SHA256:…` host key (TOFU). */
    val hostKey: String? = null,
    /**
     * Every address of this computer in the user's order (LAN IP, Tailscale
     * IP, …); [host]/[port] mirror the first. Empty on computers saved
     * before addresses existed: then [host]:[port] is the only one.
     */
    val endpoints: List<Endpoint> = emptyList(),
) {
    fun title() = name.ifBlank { host }

    /** The addresses to try (never empty for a saved computer). */
    fun addresses(): List<Endpoint> = endpoints.ifEmpty { if (host.isBlank()) emptyList() else listOf(Endpoint(host.trim(), port)) }

    /** This computer with [list] as its addresses (first one mirrored into host/port). */
    fun withAddresses(list: List<Endpoint>): Machine {
        val first = list.firstOrNull() ?: return copy(endpoints = emptyList())
        return copy(host = first.host.trim(), port = first.port, endpoints = list)
    }

    fun toJson(): JSONObject = JSONObject()
        .put("id", id).put("name", name).put("host", host).put("port", port)
        .put("user", user).put("auth", auth).put("enginePort", enginePort)
        .put("hostKey", hostKey ?: JSONObject.NULL)
        .put("endpoints", JSONArray().apply { endpoints.forEach { put(it.toJson()) } })

    companion object {
        const val AUTH_PHONE = "phone"
        const val AUTH_KEY = "key"
        const val AUTH_PASSWORD = "password"

        fun fromJson(o: JSONObject) = Machine(
            id = o.getString("id"),
            name = o.optString("name"),
            host = o.optString("host"),
            port = o.optInt("port", 22),
            user = o.optString("user"),
            auth = o.optString("auth", AUTH_PHONE),
            enginePort = o.optInt("enginePort", 27654),
            hostKey = if (o.isNull("hostKey")) null else o.optString("hostKey").ifBlank { null },
            endpoints = o.optJSONArray("endpoints")?.let { arr ->
                (0 until arr.length()).map { Endpoint.fromJson(arr.getJSONObject(it)) }.filter { it.host.isNotBlank() }
            }.orEmpty(),
        )
    }
}

/** Machine list in prefs; keys/passwords in the Keystore-backed [SecretStore]. */
class MachineStore(context: Context) {
    private val prefs = context.getSharedPreferences("zeron-machines", 0)
    val secrets = SecretStore(context)

    fun list(): List<Machine> {
        val raw = prefs.getString("machines", null) ?: return emptyList()
        return runCatching {
            val arr = JSONArray(raw)
            (0 until arr.length()).map { Machine.fromJson(arr.getJSONObject(it)) }
        }.getOrDefault(emptyList())
    }

    fun save(machine: Machine, secret: String?) {
        val all = list().filter { it.id != machine.id } + machine
        write(all)
        if (secret != null) secrets.put("machine:${machine.id}", secret)
    }

    fun update(machine: Machine) = write(list().map { if (it.id == machine.id) machine else it })

    fun delete(id: String) {
        write(list().filter { it.id != id })
        secrets.remove("machine:$id")
    }

    private fun write(all: List<Machine>) {
        val arr = JSONArray()
        all.forEach { arr.put(it.toJson()) }
        prefs.edit().putString("machines", arr.toString()).apply()
    }

    fun secret(id: String): String? = secrets.get("machine:$id")

    /**
     * One-time grouping of computers saved twice (LAN IP and Tailscale IP
     * as separate entries): see [MachineGroups]. Returns merged-away id ->
     * kept id so the caller can re-point what referred to them.
     */
    fun groupDuplicates(): Map<String, String> {
        if (prefs.getBoolean("grouped-v1", false)) return emptyMap()
        val result = MachineGroups.merge(list())
        if (result.mergedInto.isNotEmpty()) {
            for ((gone, kept) in result.mergedInto) moveSecret(gone, kept)
            write(result.machines)
        }
        prefs.edit().putBoolean("grouped-v1", true).apply()
        return result.mergedInto
    }

    /** Accounts & Computers "merge": [otherId]'s addresses join [intoId]; [otherId] goes. */
    fun merge(intoId: String, otherId: String): Machine? {
        val all = list()
        val into = all.firstOrNull { it.id == intoId } ?: return null
        val other = all.firstOrNull { it.id == otherId } ?: return null
        val merged = MachineGroups.combine(into, other)
        moveSecret(otherId, intoId)
        write(all.filter { it.id != otherId }.map { if (it.id == intoId) merged else it })
        return merged
    }

    /** "Split off": [endpoint] leaves [id] and becomes a computer of its own (same sign-in). */
    fun split(id: String, endpoint: Endpoint, name: String): Machine? {
        val all = list()
        val from = all.firstOrNull { it.id == id } ?: return null
        val rest = from.addresses().filter { it.key != endpoint.key }
        if (rest.isEmpty()) return null
        val alone = from.copy(id = UUID.randomUUID().toString(), name = name).withAddresses(listOf(endpoint))
        write(all.map { if (it.id == id) from.withAddresses(rest) else it } + alone)
        secret(id)?.let { secrets.put("machine:${alone.id}", it) }
        return alone
    }

    /** The address ([Endpoint.key]) that last worked for computer [id] on network [networkKey]. */
    fun rememberedRoute(id: String, networkKey: String): String? = prefs.getString("route:$id:$networkKey", null)

    fun rememberRoute(id: String, networkKey: String, endpointKey: String) {
        if (rememberedRoute(id, networkKey) != endpointKey) prefs.edit().putString("route:$id:$networkKey", endpointKey).apply()
    }

    /**
     * 自动选择线路 (Auto-select route): on (default), the dial order follows the network (LAN at
     * home, else Tailscale) and the link moves when the network changes; off,
     * each computer uses only the address picked for it ([pinnedAddress]).
     */
    var autoRoute: Boolean
        get() = prefs.getBoolean("auto-route", true)
        set(value) { prefs.edit().putBoolean("auto-route", value).apply() }

    /** The address picked by hand for [machine] (its first until one is picked). */
    fun pinnedAddress(machine: Machine): Endpoint = RoutePlanner.manual(machine.addresses(), prefs.getString("pin:${machine.id}", null)).first().endpoint

    fun pinAddress(id: String, endpointKey: String) {
        prefs.edit().putString("pin:$id", endpointKey).apply()
    }

    /** [machine]'s addresses in dial order for [net] (just the picked one with auto-select off). */
    fun plan(machine: Machine, net: NetworkSnapshot): List<RoutePlanner.Planned> =
        if (autoRoute) RoutePlanner.plan(machine.addresses(), net, rememberedRoute(machine.id, net.key))
        else RoutePlanner.manual(machine.addresses(), prefs.getString("pin:${machine.id}", null))

    private fun moveSecret(from: String, to: String) {
        val moved = secret(from)
        if (moved != null && secret(to) == null) secrets.put("machine:$to", moved)
        secrets.remove("machine:$from")
    }

    /** This phone's SSH identity (ed25519), generated once. Pair = (private, public). */
    fun phoneKey(): Pair<String, String> {
        val priv = secrets.get("phone-key")
        val pub = prefs.getString("phone-key-pub", null)
        if (priv != null && pub != null) return priv to pub
        val model = android.os.Build.MODEL?.replace(' ', '-') ?: "android"
        val pair = sshGenerateKey("zeron-$model")
        secrets.put("phone-key", pair.privateOpenssh)
        prefs.edit().putString("phone-key-pub", pair.publicOpenssh).apply()
        return pair.privateOpenssh to pair.publicOpenssh
    }

    /**
     * [route]: the addresses in dial order (see [RoutePlanner]); by default
     * the saved order with no head starts, i.e. one after another.
     */
    fun target(
        machine: Machine,
        hostKey: String? = machine.hostKey,
        secretOverride: String? = null,
        route: List<RoutePlanner.Planned>? = null,
    ): SshTarget {
        val auth = when (machine.auth) {
            Machine.AUTH_PASSWORD -> SshAuth.Password(secretOverride ?: secret(machine.id).orEmpty())
            Machine.AUTH_KEY -> SshAuth.Key(secretOverride ?: secret(machine.id).orEmpty(), null)
            else -> SshAuth.Key(phoneKey().first, null)
        }
        val first = route?.firstOrNull()?.endpoint ?: machine.addresses().firstOrNull() ?: Endpoint(machine.host, machine.port)
        return SshTarget(
            host = first.host.trim(),
            port = first.port.toUShort(),
            user = machine.user.trim(),
            auth = auth,
            enginePort = machine.enginePort.toUShort(),
            hostKeyFingerprint = hostKey,
            endpoints = sshEndpoints(route ?: machine.addresses().map { RoutePlanner.Planned(it, it.kind, 0, 0) }),
        )
    }
}

/** Planned addresses as the core's dial list. */
fun sshEndpoints(route: List<RoutePlanner.Planned>): List<uniffi.zeron_core.SshEndpoint> = route.map {
    uniffi.zeron_core.SshEndpoint(
        it.endpoint.host.trim(),
        it.endpoint.port.toUShort(),
        it.kind.wire,
        it.headStartMs.toUInt(),
        RoutePlanner.connectTimeoutMs(it.kind).toUInt(),
    )
}
