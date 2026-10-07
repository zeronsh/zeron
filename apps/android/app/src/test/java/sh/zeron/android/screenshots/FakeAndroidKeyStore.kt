@file:Suppress("DEPRECATION")

package sh.zeron.android.screenshots

import android.security.keystore.KeyGenParameterSpec
import java.io.InputStream
import java.io.OutputStream
import java.security.Key
import java.security.KeyStoreSpi
import java.security.Provider
import java.security.SecureRandom
import java.security.Security
import java.security.cert.Certificate
import java.security.spec.AlgorithmParameterSpec
import java.util.Collections
import java.util.Date
import java.util.Enumeration
import javax.crypto.KeyGeneratorSpi
import javax.crypto.SecretKey
import javax.crypto.spec.SecretKeySpec

/**
 * Robolectric has no "AndroidKeyStore". This in-memory stand-in lets
 * SecretStore (and so the phone's SSH key) work in screenshot tests.
 */
internal object FakeAndroidKeyStore {
    private val keys = mutableMapOf<String, SecretKey>()

    fun install() {
        if (Security.getProvider("AndroidKeyStore") != null) return
        Security.addProvider(object : Provider("AndroidKeyStore", 1.0, "test keystore") {
            init {
                put("KeyStore.AndroidKeyStore", Store::class.java.name)
                put("KeyGenerator.AES", Generator::class.java.name)
            }
        })
    }

    class Store : KeyStoreSpi() {
        override fun engineGetKey(alias: String, password: CharArray?): Key? = keys[alias]
        // Like the real AndroidKeyStore: secret-key entries need no password.
        override fun engineGetEntry(alias: String, protParam: java.security.KeyStore.ProtectionParameter?): java.security.KeyStore.Entry? =
            keys[alias]?.let { java.security.KeyStore.SecretKeyEntry(it) }
        override fun engineGetCertificateChain(alias: String): Array<Certificate>? = null
        override fun engineGetCertificate(alias: String): Certificate? = null
        override fun engineGetCreationDate(alias: String): Date = Date()
        override fun engineSetKeyEntry(alias: String, key: Key, password: CharArray?, chain: Array<out Certificate>?) {
            keys[alias] = key as SecretKey
        }
        override fun engineSetKeyEntry(alias: String, key: ByteArray, chain: Array<out Certificate>?) = Unit
        override fun engineSetCertificateEntry(alias: String, cert: Certificate) = Unit
        override fun engineDeleteEntry(alias: String) {
            keys.remove(alias)
        }
        override fun engineAliases(): Enumeration<String> = Collections.enumeration(keys.keys.toList())
        override fun engineContainsAlias(alias: String) = keys.containsKey(alias)
        override fun engineSize() = keys.size
        override fun engineIsKeyEntry(alias: String) = keys.containsKey(alias)
        override fun engineIsCertificateEntry(alias: String) = false
        override fun engineGetCertificateAlias(cert: Certificate): String? = null
        override fun engineStore(stream: OutputStream?, password: CharArray?) = Unit
        override fun engineLoad(stream: InputStream?, password: CharArray?) = Unit
    }

    class Generator : KeyGeneratorSpi() {
        private var alias = "default"
        private var bits = 256

        override fun engineInit(random: SecureRandom?) = Unit
        override fun engineInit(params: AlgorithmParameterSpec?, random: SecureRandom?) {
            (params as? KeyGenParameterSpec)?.let {
                alias = it.keystoreAlias
                if (it.keySize > 0) bits = it.keySize
            }
        }
        override fun engineInit(keysize: Int, random: SecureRandom?) {
            bits = keysize
        }
        override fun engineGenerateKey(): SecretKey {
            val bytes = ByteArray(bits / 8).also { SecureRandom().nextBytes(it) }
            return SecretKeySpec(bytes, "AES").also { keys[alias] = it }
        }
    }
}
