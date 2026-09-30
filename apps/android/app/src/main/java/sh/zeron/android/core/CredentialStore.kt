package sh.zeron.android.core

import android.content.Context
import org.json.JSONObject
import uniffi.zeron_core.AuthTokens
import uniffi.zeron_core.Credentials

/**
 * Credentials and the signed-in account's display identity, in app-private
 * storage (the client itself only knows ids).
 */
class CredentialStore(context: Context) {
    private val prefs = context.getSharedPreferences("credentials", Context.MODE_PRIVATE)

    fun stored(): Credentials? {
        val json = prefs.getString("credentials", null)?.let { runCatching { JSONObject(it) }.getOrNull() } ?: return null
        return when (json.optString("kind")) {
            "workos" -> Credentials.WorkOs(
                json.getString("userId"),
                json.getString("orgId"),
                AuthTokens(json.getString("access"), json.getString("refresh")),
            )
            "dev" -> Credentials.Dev(json.getString("userId"), json.getString("orgId"))
            else -> null
        }
    }

    /** The `AUTH_MODE=dev` edge stored [Credentials.Dev] sign in to (developer sign-in). */
    val devEdge: String? get() = prefs.getString("credentials", null)?.let { runCatching { JSONObject(it) }.getOrNull() }
        ?.takeIf { it.optString("kind") == "dev" }?.optString("edge")?.ifEmpty { null }

    fun store(credentials: Credentials, devEdge: String? = null) {
        val json = when (credentials) {
            is Credentials.WorkOs -> JSONObject()
                .put("kind", "workos")
                .put("userId", credentials.userId)
                .put("orgId", credentials.orgId)
                .put("access", credentials.tokens.accessToken)
                .put("refresh", credentials.tokens.refreshToken)
            is Credentials.Dev -> JSONObject().put("kind", "dev").put("userId", credentials.userId).put("orgId", credentials.orgId)
                .put("edge", devEdge ?: this.devEdge)
            is Credentials.Demo -> return
        }
        prefs.edit().putString("credentials", json.toString()).apply()
    }

    fun updateTokens(tokens: AuthTokens) {
        val current = stored() as? Credentials.WorkOs ?: return
        store(Credentials.WorkOs(current.userId, current.orgId, tokens))
    }

    fun clear() {
        prefs.edit().clear().apply()
    }

    data class Profile(val name: String? = null, val email: String? = null, val orgName: String? = null)

    var profile: Profile
        get() = Profile(prefs.getString("name", null), prefs.getString("email", null), prefs.getString("orgName", null))
        set(value) {
            prefs.edit().putString("name", value.name).putString("email", value.email).putString("orgName", value.orgName).apply()
        }
}
