package sh.zeron.runtime

import org.json.JSONObject
import java.io.File
import java.security.SecureRandom

internal data class Secrets(val edgeToken: String, val ipcToken: String)

/** What bootstrap has done so far, persisted as runtime/state.json. */
internal data class PersistedState(
    val bootstrapVersion: Int = 0,
    val rootfsRelease: String? = null,
    val packages: String? = null,
    val secrets: Secrets? = null,
    val customServer: CustomServer? = null,
)

internal class StateStore(private val file: File) {
    @Synchronized
    fun read(): PersistedState {
        if (!file.exists()) return PersistedState()
        return try {
            val json = JSONObject(file.readText())
            PersistedState(
                bootstrapVersion = json.optInt("bootstrapVersion", 0),
                rootfsRelease = json.optString("rootfsRelease").ifEmpty { null },
                packages = json.optString("packages").ifEmpty { null },
                secrets = json.optJSONObject("secrets")?.let {
                    Secrets(it.getString("edgeToken"), it.getString("ipcToken"))
                },
                customServer = json.optJSONObject("customServer")?.let {
                    CustomServer(it.getString("edgeUrl"), it.getString("token"))
                },
            )
        } catch (_: Exception) {
            PersistedState()
        }
    }

    @Synchronized
    fun update(transform: (PersistedState) -> PersistedState): PersistedState {
        val next = transform(read())
        val json = JSONObject()
            .put("bootstrapVersion", next.bootstrapVersion)
            .put("rootfsRelease", next.rootfsRelease ?: "")
            .put("packages", next.packages ?: "")
        next.secrets?.let {
            json.put("secrets", JSONObject().put("edgeToken", it.edgeToken).put("ipcToken", it.ipcToken))
        }
        next.customServer?.let {
            json.put("customServer", JSONObject().put("edgeUrl", it.edgeUrl).put("token", it.token))
        }
        file.parentFile?.mkdirs()
        // Write-then-rename: a crash mid-write must not lose the secrets the
        // client already holds.
        val tmp = File(file.path + ".tmp")
        tmp.writeText(json.toString(2))
        if (!tmp.renameTo(file)) error("could not replace $file")
        return next
    }

    /** The two loopback bearers, generated once (docs/android.md § Engine switches). */
    fun secrets(): Secrets =
        read().secrets ?: update { it.copy(secrets = Secrets(newSecret(), newSecret())) }.secrets!!

    private fun newSecret(): String {
        val bytes = ByteArray(32).also { SecureRandom().nextBytes(it) }
        return bytes.joinToString("") { "%02x".format(it) }
    }
}
