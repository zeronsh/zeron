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
) {
    fun title() = name.ifBlank { host }

    fun toJson(): JSONObject = JSONObject()
        .put("id", id).put("name", name).put("host", host).put("port", port)
        .put("user", user).put("auth", auth).put("enginePort", enginePort)
        .put("hostKey", hostKey ?: JSONObject.NULL)

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

    fun target(machine: Machine, hostKey: String? = machine.hostKey, secretOverride: String? = null): SshTarget {
        val auth = when (machine.auth) {
            Machine.AUTH_PASSWORD -> SshAuth.Password(secretOverride ?: secret(machine.id).orEmpty())
            Machine.AUTH_KEY -> SshAuth.Key(secretOverride ?: secret(machine.id).orEmpty(), null)
            else -> SshAuth.Key(phoneKey().first, null)
        }
        return SshTarget(
            host = machine.host.trim(),
            port = machine.port.toUShort(),
            user = machine.user.trim(),
            auth = auth,
            enginePort = machine.enginePort.toUShort(),
            hostKeyFingerprint = hostKey,
        )
    }
}
