package sh.zeron.android.tools

import android.net.Uri
import android.util.Base64
import android.webkit.MimeTypeMap
import android.webkit.WebResourceResponse
import kotlinx.coroutines.runBlocking
import org.json.JSONObject
import sh.zeron.android.core.AppModel
import java.io.ByteArrayInputStream
import java.io.InputStream
import java.util.concurrent.ConcurrentHashMap

/**
 * The in-app browser's plumbing.
 *
 * - **Workspace pages**: `https://<token>.workspace.zeron.invalid/<path>`
 *   serves a workspace's files straight from its device (`ReadWorkspaceBytes`,
 *   streamed), so an agent's `index.html` opens with its CSS, scripts and
 *   images — from whichever computer owns it. The token is stable per
 *   workspace, so its origin (and localStorage) is too.
 * - **Previews**: dev servers the session's device discovered
 *   (`WatchPreviews`). The app has no preview proxy, so they open straight
 *   from the device over the network (`http://<its address>:<port>`).
 */
object Browser {
    const val WORKSPACE_DOMAIN = "workspace.zeron.invalid"
    private val refs = ConcurrentHashMap<String, WorkspaceRef>()

    fun token(ref: WorkspaceRef): String {
        val key = "${ref.deviceId}|${ref.chatId ?: ""}|${ref.spaceId ?: ""}"
        val token = "w" + Integer.toHexString(key.hashCode()).padStart(8, '0')
        refs[token] = ref
        return token
    }

    /** A host (or `host:port`-less address) typed for previews, or null. */
    fun previewHost(address: String): String? {
        val host = address.trim().removePrefix("http://").removePrefix("https://").trimEnd('/').substringBefore('/')
        return host.takeIf { it.isNotEmpty() && it.all { c -> c.isLetterOrDigit() || c in ".-:[]" } }
    }

    fun workspaceUrl(ref: WorkspaceRef, path: String): String =
        "https://${token(ref)}.$WORKSPACE_DOMAIN/" + path.split('/').joinToString("/") { Uri.encode(it) }

    /** The workspace and relative path a workspace URL names. */
    fun resolve(uri: Uri): Pair<WorkspaceRef, String>? {
        val host = uri.host ?: return null
        if (!host.endsWith(".$WORKSPACE_DOMAIN")) return null
        val ref = refs[host.removeSuffix(".$WORKSPACE_DOMAIN")] ?: return null
        val path = (uri.path ?: "/").trimStart('/')
        return ref to path
    }

    fun isWorkspace(url: String?): Boolean = url?.let { runCatching { Uri.parse(it).host?.endsWith(".$WORKSPACE_DOMAIN") }.getOrNull() } == true

    /** A human URL for the address bar: workspace pages show their path. */
    fun display(url: String?): String {
        if (url.isNullOrEmpty()) return ""
        val uri = Uri.parse(url)
        resolve(uri)?.let { (ref, path) -> return "${ref.title}/$path" }
        return url.removePrefix("https://").removePrefix("http://").trimEnd('/')
    }

    private val loopbackHost = Regex("^(localhost|[a-z0-9-]+(\\.[a-z0-9-]+)*\\.localhost|127(\\.\\d{1,3}){3}|\\[::1]|10\\.0\\.2\\.2)(:\\d+)?(/.*)?$", RegexOption.IGNORE_CASE)

    /**
     * What the address bar typed means (desktop `normalize_address`): a URL
     * as is, loopback hosts over http, other hosts over https, anything else a
     * web search.
     */
    fun normalize(input: String): String {
        val text = input.trim()
        if (text.isEmpty()) return "about:blank"
        if (Regex("^[a-z][a-z0-9+.-]*://", RegexOption.IGNORE_CASE).containsMatchIn(text) || text.startsWith("about:")) return text
        if (Regex("^\\d+$").matches(text)) return "http://localhost:$text"
        if (loopbackHost.matches(text)) return "http://$text"
        val host = text.substringBefore('/')
        if (!text.contains(' ') && (host.contains('.') || host.contains(':'))) return "https://$text"
        return "https://duckduckgo.com/?q=" + Uri.encode(text)
    }

    /** Serve a workspace URL (called on the WebView's IO thread). */
    fun intercept(model: AppModel, uri: Uri): WebResourceResponse? {
        val (ref, raw) = resolve(uri) ?: return null
        val path = raw.removeSuffix("/")
        val candidates = if (raw.isEmpty() || raw.endsWith("/")) listOf(if (path.isEmpty()) "index.html" else "$path/index.html") else listOf(path, "$path/index.html")
        for (candidate in candidates) {
            val stream = runCatching { WorkspaceStream.open(model, ref, candidate) }.getOrNull() ?: continue
            val mime = mimeOf(candidate)
            val text = mime.startsWith("text/") || mime in setOf("application/javascript", "application/json", "image/svg+xml")
            return WebResourceResponse(mime, if (text) "utf-8" else null, 200, "OK", mapOf("Cache-Control" to "no-cache", "Access-Control-Allow-Origin" to "*"), stream)
        }
        val body = "<html><body style=\"font-family:sans-serif;padding:24px\"><h3>Not found</h3><p>${path.ifEmpty { "index.html" }} isn't in ${ref.title}.</p></body></html>"
        return WebResourceResponse("text/html", "utf-8", 404, "Not Found", emptyMap(), ByteArrayInputStream(body.toByteArray()))
    }

    fun mimeOf(path: String): String {
        val ext = path.substringAfterLast('.', "").lowercase()
        return when (ext) {
            "html", "htm" -> "text/html"
            "js", "mjs", "cjs" -> "application/javascript"
            "css" -> "text/css"
            "json", "map" -> "application/json"
            "svg" -> "image/svg+xml"
            "wasm" -> "application/wasm"
            "md" -> "text/plain"
            else -> MimeTypeMap.getSingleton().getMimeTypeFromExtension(ext) ?: "application/octet-stream"
        }
    }

    /** One `WatchPreviews` service. */
    data class Preview(val id: String, val name: String, val hostname: String, val port: Int, val deviceName: String?, val projectName: String?) {
        fun url(proxyPort: Int) = "http://$hostname:$proxyPort"
    }

    data class Previews(val services: List<Preview>, val proxyPort: Int, val error: String?)

    fun previews(frame: JSONObject): Previews {
        val list = frame.optJSONArray("services")
        val services = (0 until (list?.length() ?: 0)).map {
            val s = list!!.getJSONObject(it)
            Preview(
                id = s.optString("id"),
                name = s.optString("name").ifEmpty { s.optString("hostname") },
                hostname = s.optString("hostname"),
                port = s.optInt("port"),
                deviceName = s.optString("deviceName").ifEmpty { null },
                projectName = s.optString("projectName").ifEmpty { null },
            )
        }
        return Previews(services, frame.optInt("proxyPort", 7331), frame.optString("error").ifEmpty { null }.takeIf { !frame.isNull("error") })
    }
}

/**
 * A workspace file as an [InputStream] that pulls `ReadWorkspaceBytes`
 * chunks on demand (WebView reads responses on its own IO threads). The first
 * chunk is fetched by [open], so a missing file fails there, not mid-body.
 */
class WorkspaceStream private constructor(
    private val model: AppModel,
    private val ref: WorkspaceRef,
    private val path: String,
    first: JSONObject,
) : InputStream() {
    private var buffer: ByteArray = Base64.decode(first.optString("data"), Base64.DEFAULT)
    private var pos = 0
    private var next = first.optLong("nextOffset")
    private var done = first.optBoolean("done")
    private val revision = first.optString("revision")

    private fun fill(): Boolean {
        while (pos >= buffer.size) {
            if (done) return false
            val o = runBlocking { chunk(model, ref, path, next, revision) }
            buffer = Base64.decode(o.optString("data"), Base64.DEFAULT)
            pos = 0
            next = o.optLong("nextOffset", next + buffer.size)
            done = o.optBoolean("done") || buffer.isEmpty()
        }
        return true
    }

    override fun read(): Int = if (fill()) buffer[pos++].toInt() and 0xFF else -1

    override fun read(b: ByteArray, off: Int, len: Int): Int {
        if (len == 0) return 0
        if (!fill()) return -1
        val n = minOf(len, buffer.size - pos)
        System.arraycopy(buffer, pos, b, off, n)
        pos += n
        return n
    }

    override fun available(): Int = buffer.size - pos

    companion object {
        private suspend fun chunk(model: AppModel, ref: WorkspaceRef, path: String, offset: Long, revision: String?): JSONObject {
            val params = ref.target().put("path", path).put("offset", offset)
            if (revision != null) params.put("expectedRevision", revision)
            return model.hostCall(ref.deviceId, WorkspaceApi.READ_BYTES, params) as JSONObject
        }

        fun open(model: AppModel, ref: WorkspaceRef, path: String): WorkspaceStream =
            WorkspaceStream(model, ref, path, runBlocking { chunk(model, ref, path, 0, null) })
    }
}
