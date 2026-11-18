package sh.zeron.android.core

import java.net.URI
import java.net.URLDecoder

/**
 * `zeron://server?address=<address>`: an invite to a self-hosted server. The
 * address is what a person would type (`zeron.acme.com`,
 * `https://zeron.acme.com`), percent-encoded. A link only fills in the server
 * sheet and looks the address up: choosing it still takes a tap.
 */
object ServerLink {
    const val HOST = "server"

    /** The link's address; null for any other link, or one without an address (ignored, not an error). */
    fun address(uri: String): String? {
        val link = runCatching { URI(uri) }.getOrNull() ?: return null
        if (!link.scheme.equals("zeron", ignoreCase = true) || !link.host.equals(HOST, ignoreCase = true)) return null
        return link.rawQuery.orEmpty().split('&').firstNotNullOfOrNull { pair ->
            val key = pair.substringBefore('=')
            if (pair.contains('=') && decode(key) == "address") decode(pair.substringAfter('='))?.trim()?.takeIf { it.isNotEmpty() } else null
        }
    }

    // (The String overload: the Charset one is API 33.) A broken escape is no address.
    private fun decode(part: String): String? = runCatching { URLDecoder.decode(part, "UTF-8") }.getOrNull()
}
