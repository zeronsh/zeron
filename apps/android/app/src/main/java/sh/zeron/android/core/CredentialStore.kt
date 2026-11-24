package sh.zeron.android.core

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import org.json.JSONObject
import uniffi.zeron_core.AuthTokens
import uniffi.zeron_core.Credentials

/**
 * Credentials and the signed-in account's display identity, in app-private
 * storage (the client itself only knows ids). Tokens are sealed with an
 * Android Keystore key (the iOS Keychain's counterpart): the key never
 * leaves the keystore, so a copied preferences file can't be replayed.
 */
class CredentialStore(context: Context) {
    private val prefs = context.getSharedPreferences("credentials", Context.MODE_PRIVATE)

    private fun key(): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey(KEY_ALIAS, null) as? SecretKey)?.let { return it }
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        generator.init(
            KeyGenParameterSpec.Builder(KEY_ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .build(),
        )
        return generator.generateKey()
    }

    private fun seal(plain: String): String {
        val cipher = Cipher.getInstance(TRANSFORMATION).apply { init(Cipher.ENCRYPT_MODE, key()) }
        val sealed = cipher.iv + cipher.doFinal(plain.toByteArray())
        return Base64.encodeToString(sealed, Base64.NO_WRAP)
    }

    private fun open(sealed: String): String? = runCatching {
        val bytes = Base64.decode(sealed, Base64.NO_WRAP)
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, bytes, 0, 12))
        String(cipher.doFinal(bytes, 12, bytes.size - 12))
    }.getOrNull()

    private fun storedJson(): String? {
        prefs.getString("sealed", null)?.let { return open(it) }
        // Builds before the keystore stored plain JSON: seal it on first read.
        val legacy = prefs.getString("credentials", null) ?: return null
        prefs.edit().putString("sealed", seal(legacy)).remove("credentials").apply()
        return legacy
    }

    /** Credentials and the server they were signed in to (null: Zeron's own). */
    data class Account(val credentials: Credentials, val server: StoredServer?)

    fun account(): Account? = storedJson()?.let(CredentialJson::decode)

    fun stored(): Credentials? = account()?.credentials

    fun store(credentials: Credentials, server: StoredServer?) {
        val json = CredentialJson.encode(credentials, server) ?: return
        prefs.edit().putString("sealed", seal(json)).remove("credentials").apply()
    }

    /** Refreshed tokens, kept with the server they came from. */
    fun updateTokens(tokens: AuthTokens) {
        val account = account() ?: return
        val current = account.credentials as? Credentials.WorkOs ?: return
        store(Credentials.WorkOs(current.userId, current.orgId, tokens), account.server)
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

    private companion object {
        const val KEY_ALIAS = "zeron-credentials"
        const val TRANSFORMATION = "AES/GCM/NoPadding"
    }
}

/**
 * The sealed credentials JSON: `{kind, userId, orgId, access, refresh}`, plus
 * `edge`/`workosClientId`/`serverName` for an account on a server other than
 * Zeron's. Without them it is Zeron's own, as every install before them.
 */
internal object CredentialJson {
    fun encode(credentials: Credentials, server: StoredServer?): String? {
        val json = when (credentials) {
            is Credentials.WorkOs -> JSONObject()
                .put("kind", "workos")
                .put("userId", credentials.userId)
                .put("orgId", credentials.orgId)
                .put("access", credentials.tokens.accessToken)
                .put("refresh", credentials.tokens.refreshToken)
            is Credentials.Dev -> JSONObject().put("kind", "dev").put("userId", credentials.userId).put("orgId", credentials.orgId)
            is Credentials.Demo -> return null
        }
        server?.let {
            json.put("edge", it.url).put("workosClientId", it.workosClientId)
            it.name?.let { name -> json.put("serverName", name) }
        }
        return json.toString()
    }

    fun decode(text: String): CredentialStore.Account? {
        val json = runCatching { JSONObject(text) }.getOrNull() ?: return null
        val credentials = when (json.optString("kind")) {
            "workos" -> Credentials.WorkOs(
                json.getString("userId"),
                json.getString("orgId"),
                AuthTokens(json.getString("access"), json.getString("refresh")),
            )
            "dev" -> Credentials.Dev(json.getString("userId"), json.getString("orgId"))
            else -> return null
        }
        val edge = json.stringOrNull("edge")
        val client = json.stringOrNull("workosClientId")
        val server = if (edge != null && client != null) StoredServer(edge, client, json.stringOrNull("serverName")) else null
        return CredentialStore.Account(credentials, server)
    }
}
