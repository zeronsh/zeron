package sh.zeron.android.update

import java.io.OutputStream
import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket
import java.util.concurrent.Executors
import kotlin.concurrent.thread

/**
 * Minimal HTTP/1.1 server for updater tests (the Android compile classpath
 * has no com.sun.net.httpserver). One request per connection.
 */
class TinyHttpServer {
    class Request(val path: String, val headers: Map<String, String>) {
        fun header(name: String): String? = headers[name.lowercase()]
    }

    class Response(private val out: OutputStream) {
        fun head(code: Int, length: Long? = null, extra: Map<String, String> = emptyMap()) {
            val sb = StringBuilder("HTTP/1.1 $code X\r\nConnection: close\r\n")
            if (length != null) sb.append("Content-Length: $length\r\n")
            extra.forEach { (k, v) -> sb.append("$k: $v\r\n") }
            sb.append("\r\n")
            out.write(sb.toString().toByteArray())
            out.flush()
        }
        fun body(bytes: ByteArray, off: Int = 0, len: Int = bytes.size - off) { out.write(bytes, off, len); out.flush() }
    }

    private val socket = ServerSocket(0, 50, InetAddress.getByName("127.0.0.1"))
    private val pool = Executors.newCachedThreadPool()
    private val routes = mutableMapOf<String, (Request, Response) -> Unit>()
    val port: Int get() = socket.localPort

    fun route(path: String, handler: (Request, Response) -> Unit) { routes[path] = handler }

    fun start() = apply {
        thread(isDaemon = true) {
            while (!socket.isClosed) {
                val s = runCatching { socket.accept() }.getOrNull() ?: break
                pool.execute { handle(s) }
            }
        }
    }

    private fun handle(s: Socket) = s.use {
        runCatching {
            val input = s.getInputStream().bufferedReader(Charsets.ISO_8859_1)
            val line = input.readLine() ?: return@runCatching
            val path = line.split(' ').getOrNull(1) ?: "/"
            val headers = mutableMapOf<String, String>()
            while (true) {
                val h = input.readLine() ?: break
                if (h.isEmpty()) break
                headers[h.substringBefore(':').trim().lowercase()] = h.substringAfter(':').trim()
            }
            // Exact path, else the longest route that prefixes it (mirror-style URLs).
            val handler = routes[path] ?: routes.entries.filter { path.startsWith(it.key) }.maxByOrNull { it.key.length }?.value
                ?: { _, r -> r.head(404, 0) }
            handler(Request(path, headers), Response(s.getOutputStream()))
        }
    }

    fun stop() { socket.close(); pool.shutdownNow() }
}
