package sh.zeron.runtime

import android.util.Base64
import java.net.InetSocketAddress
import java.net.Socket
import java.security.SecureRandom

/**
 * Loopback probes from the app process. Plain sockets rather than
 * HttpURLConnection: the app's network security config may forbid cleartext
 * HTTP, and this is only ever 127.0.0.1.
 */
internal object Health {
    private const val TIMEOUT_MS = 2_000

    /**
     * HTTP status of an authenticated WebSocket upgrade on the engine's IPC
     * port (101 = up and our token accepted, 401 = token rejected), or null
     * if nothing answers. A bare TCP connect would also tell us the port is
     * open, but the engine logs every aborted handshake as a warning; a real
     * handshake followed by a close frame is silent.
     */
    fun ipcStatus(port: Int, token: String): Int? = try {
        Socket().use { socket ->
            socket.connect(InetSocketAddress("127.0.0.1", port), TIMEOUT_MS)
            socket.soTimeout = TIMEOUT_MS
            val key = Base64.encodeToString(ByteArray(16).also { SecureRandom().nextBytes(it) }, Base64.NO_WRAP)
            val out = socket.getOutputStream()
            out.write(
                ("GET / HTTP/1.1\r\nHost: 127.0.0.1:$port\r\nUpgrade: websocket\r\n" +
                    "Connection: Upgrade\r\nSec-WebSocket-Key: $key\r\nSec-WebSocket-Version: 13\r\n" +
                    "Authorization: Bearer $token\r\n\r\n").toByteArray(),
            )
            val status = readLine(socket).split(' ').getOrNull(1)?.toIntOrNull()
            if (status == 101) {
                // Masked close frame, code 1000 (clients must mask; RFC 6455 §5.3).
                val mask = ByteArray(4).also { SecureRandom().nextBytes(it) }
                val payload = byteArrayOf(0x03, 0xE8.toByte())
                out.write(byteArrayOf(0x88.toByte(), 0x82.toByte()) + mask +
                    ByteArray(2) { (payload[it].toInt() xor mask[it].toInt()).toByte() })
                out.flush()
            }
            status
        }
    } catch (_: Exception) {
        null
    }

    // The status line only; unbuffered so nothing else is consumed.
    private fun readLine(socket: Socket): String {
        val input = socket.getInputStream()
        val sb = StringBuilder()
        while (sb.length < 1024) {
            val c = input.read()
            if (c < 0 || c == '\n'.code) break
            if (c != '\r'.code) sb.append(c.toChar())
        }
        return sb.toString()
    }
}
