package sh.zeron.android.core

import org.json.JSONObject
import uniffi.zeron_core.ServerInfo
import uniffi.zeron_core.serverInfo

/**
 * A server other than Zeron's own as the app keeps it (beside the
 * credentials, or as the one chosen to sign in to): what [serverInfo] needs
 * to describe it again. Zeron's own is kept as nothing at all.
 */
data class StoredServer(val url: String, val workosClientId: String, val name: String? = null) {
    fun info(): ServerInfo = serverInfo(url, workosClientId, name)

    /** `{url, workosClientId, name?}`: the chosen server (`settings` key `server`). */
    fun toJson(): String = JSONObject()
        .put("url", url)
        .put("workosClientId", workosClientId)
        .apply { name?.let { put("name", it) } }
        .toString()

    companion object {
        /** Null for Zeron's own. */
        fun of(server: ServerInfo): StoredServer? =
            if (server.isOfficial) null else StoredServer(server.url, server.workosClientId, server.name)

        fun fromJson(text: String?): StoredServer? {
            val json = text?.let { runCatching { JSONObject(it) }.getOrNull() } ?: return null
            val url = json.stringOrNull("url") ?: return null
            val client = json.stringOrNull("workosClientId") ?: return null
            return StoredServer(url, client, json.stringOrNull("name"))
        }
    }
}

/**
 * The server the app uses: a launch argument's wins; signed in, the account's
 * own (none stored is [official]); signed out, the one chosen to sign in to.
 */
fun <T : Any> pickServer(override: T?, signedIn: Boolean, account: T?, chosen: T?, official: T): T =
    override ?: (if (signedIn) account else chosen) ?: official

/**
 * Whose docs the core dir holds (its `.owner`). Zeron's own server keeps the
 * bare `user/org` it always had, so existing caches survive.
 */
fun coreOwner(customServerUrl: String?, userId: String, orgId: String): String =
    if (customServerUrl == null) "$userId/$orgId" else "$customServerUrl|$userId/$orgId"

/** The same sign-in: one address, one WorkOS client (a renamed server is still the one signed in to). */
fun ServerInfo.isSameAs(other: ServerInfo): Boolean = url == other.url && workosClientId == other.workosClientId

/** A string field, absent or JSON `null` alike. */
internal fun JSONObject.stringOrNull(key: String): String? =
    if (has(key) && !isNull(key)) getString(key).takeIf { it.isNotEmpty() } else null
