package sh.zeron.android.tools

import android.util.Base64
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import org.json.JSONArray
import org.json.JSONObject
import sh.zeron.android.core.AppModel
import uniffi.zeron_core.CoreClient
import uniffi.zeron_core.CoreException
import uniffi.zeron_core.HostStream
import uniffi.zeron_core.HostStreamListener

/**
 * One workspace the developer tools work on: a chat's folder (or a
 * project's) on a device — this phone's engine or any other. Every call is a
 * host RPC to [deviceId], so the tools work the same for the phone's own
 * projects and a computer's (docs/android.md § Developer tools).
 */
data class WorkspaceRef(
    val deviceId: String,
    val chatId: String?,
    val spaceId: String?,
    /** Absolute folder on the device, when known (SendFiles, display). */
    val root: String?,
    val title: String,
    val deviceName: String?,
) {
    /** The flattened `WorkspaceTarget` the engine resolves. */
    fun target(): JSONObject = JSONObject().apply {
        if (chatId != null) put("chatId", chatId) else put("spaceId", spaceId)
    }

    fun absolute(path: String): String? = root?.let { if (path.isEmpty()) it else it.trimEnd('/') + "/" + path }
}

/** `ListWorkspaceDirectory` entry. */
data class Entry(
    val path: String,
    val name: String,
    val isDir: Boolean,
    val size: Long?,
    val ignored: Boolean,
    val symlink: Boolean,
)

/** `ReadWorkspaceFile`: the text and what a save must quote back. */
data class FileText(
    val path: String,
    val checkoutId: String,
    val text: String?,
    val contentHash: String?,
    val size: Long,
    val encoding: String,
    val lineEnding: String?,
    val readOnlyReason: String?,
    val truncated: Boolean,
) {
    val binary: Boolean get() = encoding == "binary"
    val editable: Boolean get() = text != null && readOnlyReason == null && contentHash != null
}

sealed interface SaveResult {
    data class Written(val contentHash: String) : SaveResult
    /** `changed` / `deleted` / `replaced` / `notRegularFile`. */
    data class Conflict(val reason: String) : SaveResult
}

/** One `WatchWorkspaceGitStatus` file: index + worktree states. */
data class GitMark(val letter: Char, val kind: Kind) {
    /** Ascending strength: a folder shows its strongest descendant. */
    enum class Kind { Added, Untracked, Modified, Renamed, Deleted, Conflict }
}

/** The engine's workspace file methods over host RPC. */
class WorkspaceApi(private val model: AppModel) {
    private fun client(): CoreClient = model.client.value ?: throw IllegalStateException("Not connected")

    private suspend fun call(ref: WorkspaceRef, method: String, params: JSONObject): Any =
        model.hostCall(ref.deviceId, method, params)

    private fun params(ref: WorkspaceRef, vararg pairs: Pair<String, Any?>): JSONObject =
        ref.target().apply { for ((k, v) in pairs) if (v != null) put(k, v) }

    /** Every entry of one directory (`""` = the root), following pages. */
    suspend fun list(ref: WorkspaceRef, dir: String, includeIgnored: Boolean): List<Entry> {
        val out = ArrayList<Entry>()
        var cursor: String? = null
        do {
            val reply = call(ref, LIST, params(ref, "directory" to dir, "includeIgnored" to includeIgnored, "cursor" to cursor)) as JSONObject
            val entries = reply.optJSONArray("entries") ?: JSONArray()
            for (i in 0 until entries.length()) out += entry(entries.getJSONObject(i))
            cursor = reply.optString("nextCursor").ifEmpty { null }.takeIf { !reply.isNull("nextCursor") }
        } while (cursor != null && out.size < MAX_LISTED)
        return out.sortedWith(compareBy<Entry>({ !it.isDir }, { it.name.lowercase() }))
    }

    suspend fun search(ref: WorkspaceRef, query: String, includeIgnored: Boolean): List<Entry> {
        val reply = call(ref, SEARCH, params(ref, "query" to query, "includeIgnored" to includeIgnored, "limit" to 100)) as? JSONArray ?: return emptyList()
        return (0 until reply.length()).map { entry(reply.getJSONObject(it)) }
    }

    suspend fun read(ref: WorkspaceRef, path: String): FileText {
        val o = call(ref, READ, params(ref, "path" to path)) as JSONObject
        return FileText(
            path = o.optString("path", path),
            checkoutId = o.optString("checkoutId"),
            text = if (o.isNull("text")) null else o.optString("text"),
            contentHash = o.optString("contentHash").ifEmpty { null }.takeIf { !o.isNull("contentHash") },
            size = o.optLong("size"),
            encoding = o.optString("encoding", "utf8"),
            lineEnding = o.optString("lineEnding").ifEmpty { null },
            readOnlyReason = o.optString("readOnlyReason").ifEmpty { null }.takeIf { !o.isNull("readOnlyReason") },
            truncated = o.optBoolean("truncated"),
        )
    }

    /** Optimistic save: the engine writes only if the file still has [base]'s hash. */
    suspend fun write(ref: WorkspaceRef, base: FileText, text: String): SaveResult {
        val o = call(
            ref,
            WRITE,
            params(
                ref,
                "expectedCheckoutId" to base.checkoutId,
                "path" to base.path,
                "text" to text.replace("\r\n", "\n").replace('\r', '\n'),
                "expectedContentHash" to base.contentHash,
                "encoding" to if (base.encoding == "utf8Bom") "utf8Bom" else "utf8",
                "lineEnding" to if (base.lineEnding == "crlf") "crlf" else "lf",
            ),
        ) as JSONObject
        return when (o.optString("status")) {
            "written" -> SaveResult.Written(o.optJSONObject("file")?.optString("contentHash").orEmpty())
            else -> SaveResult.Conflict(o.optString("reason", "changed"))
        }
    }

    /**
     * Raw bytes of any workspace file, chunk by chunk (`ReadWorkspaceBytes`),
     * never held whole. [onChunk] gets each piece and the total size.
     */
    suspend fun readBytes(ref: WorkspaceRef, path: String, onChunk: (ByteArray, Long) -> Unit) {
        var offset = 0L
        var revision: String? = null
        while (true) {
            val o = call(ref, READ_BYTES, params(ref, "path" to path, "offset" to offset, "expectedRevision" to revision)) as JSONObject
            revision = o.optString("revision")
            val size = o.optLong("size")
            val data = Base64.decode(o.optString("data"), Base64.DEFAULT)
            onChunk(data, size)
            offset = o.optLong("nextOffset", offset + data.size)
            if (o.optBoolean("done") || data.isEmpty()) return
        }
    }

    suspend fun bytes(ref: WorkspaceRef, path: String, limit: Long = 64L shl 20): ByteArray {
        val out = java.io.ByteArrayOutputStream()
        readBytes(ref, path) { chunk, size ->
            if (size > limit) throw IllegalStateException("File is too large to preview (${formatBytes(size)}).")
            out.write(chunk)
        }
        return out.toByteArray()
    }

    /**
     * A host stream delivered on the main thread. Cancel the returned handle
     * (or let the screen dispose it) to stop the device's side too.
     */
    suspend fun watch(
        deviceId: String,
        method: String,
        params: JSONObject,
        scope: CoroutineScope,
        onItem: (JSONObject) -> Unit,
        onEnd: () -> Unit = {},
    ): HostStream = client().hostWatch(
        deviceId,
        method,
        params.toString(),
        object : HostStreamListener {
            override fun onItem(json: String) {
                val item = runCatching { JSONObject(json) }.getOrNull() ?: return
                scope.launch(Dispatchers.Main) { onItem(item) }
            }

            override fun onEnd() {
                scope.launch(Dispatchers.Main) { onEnd() }
            }
        },
    )

    suspend fun watchFiles(ref: WorkspaceRef, scope: CoroutineScope, onItem: (JSONObject) -> Unit, onEnd: () -> Unit) =
        watch(ref.deviceId, WATCH_FILES, ref.target(), scope, onItem, onEnd)

    suspend fun watchGit(ref: WorkspaceRef, scope: CoroutineScope, onItem: (Map<String, GitMark>?) -> Unit, onEnd: () -> Unit) =
        watch(ref.deviceId, WATCH_GIT, ref.target(), scope, { onItem(gitMarks(it)) }, onEnd)

    companion object {
        const val LIST = "ListWorkspaceDirectory"
        const val SEARCH = "SearchWorkspaceFiles"
        const val READ = "ReadWorkspaceFile"
        const val READ_BYTES = "ReadWorkspaceBytes"
        const val WRITE = "WriteWorkspaceFile"
        const val WATCH_FILES = "WatchWorkspaceFiles"
        const val WATCH_GIT = "WatchWorkspaceGitStatus"
        const val WATCH_PREVIEWS = "WatchPreviews"
        const val SEND_FILES = "SendFiles"
        private const val MAX_LISTED = 20_000

        fun entry(o: JSONObject): Entry {
            val kind = o.optString("kind")
            return Entry(
                path = o.optString("path"),
                name = o.optString("name").ifEmpty { o.optString("path").substringAfterLast('/') },
                isDir = kind == "directory",
                size = if (o.has("size") && !o.isNull("size")) o.optLong("size") else null,
                ignored = o.optBoolean("ignored"),
                symlink = kind == "symlink",
            )
        }

        /** `{status: {files: [...]}}` → path → mark; null when git is unavailable. */
        fun gitMarks(frame: JSONObject): Map<String, GitMark>? {
            val status = frame.optJSONObject("status") ?: return null
            val files = status.optJSONArray("files") ?: return emptyMap()
            val out = HashMap<String, GitMark>()
            for (i in 0 until files.length()) {
                val f = files.getJSONObject(i)
                gitMark(f.optString("index"), f.optString("worktree"))?.let { out[f.optString("path")] = it }
            }
            return out
        }

        /** Desktop's decoration order: conflict, deleted, renamed, modified, untracked, added. */
        fun gitMark(index: String, worktree: String): GitMark? {
            val states = setOf(index, worktree)
            return when {
                "unmerged" in states -> GitMark('!', GitMark.Kind.Conflict)
                "deleted" in states -> GitMark('D', GitMark.Kind.Deleted)
                "renamed" in states || "copied" in states -> GitMark('R', GitMark.Kind.Renamed)
                "modified" in states || "typeChanged" in states -> GitMark('M', GitMark.Kind.Modified)
                "untracked" in states -> GitMark('U', GitMark.Kind.Untracked)
                "added" in states -> GitMark('A', GitMark.Kind.Added)
                else -> null
            }
        }

        /** Directories inherit the strongest mark of anything under them. */
        fun folderMarks(marks: Map<String, GitMark>): Map<String, GitMark> {
            val out = HashMap<String, GitMark>()
            for ((path, mark) in marks) {
                var dir = path.substringBeforeLast('/', "")
                while (dir.isNotEmpty()) {
                    val prev = out[dir]
                    if (prev == null || mark.kind.ordinal > prev.kind.ordinal) out[dir] = mark
                    dir = dir.substringBeforeLast('/', "")
                }
            }
            return out
        }

        fun isUnsupported(e: Throwable) = e is CoreException.Unsupported
    }
}

fun formatBytes(bytes: Long): String = when {
    bytes < 1024 -> "$bytes B"
    bytes < 1024 * 1024 -> "%.1f KB".format(bytes / 1024.0)
    bytes < 1024L * 1024 * 1024 -> "%.1f MB".format(bytes / (1024.0 * 1024))
    else -> "%.2f GB".format(bytes / (1024.0 * 1024 * 1024))
}

/** What the viewer does with a file, by name. */
enum class FileKind { Text, Markdown, Html, Image, Svg, Pdf, Binary;

    companion object {
        private val images = setOf("png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "heic", "heif", "avif")
        private val binaries = setOf(
            "zip", "gz", "tgz", "xz", "bz2", "7z", "rar", "jar", "apk", "aab", "so", "a", "o", "dylib", "dll", "exe",
            "class", "wasm", "mp3", "mp4", "m4a", "mov", "webm", "ogg", "wav", "flac", "ttf", "otf", "woff", "woff2",
            "sqlite", "db", "bin", "dmg", "iso", "psd", "xlsx", "docx", "pptx", "keynote",
        )

        fun of(path: String): FileKind {
            val ext = path.substringAfterLast('/').substringAfterLast('.', "").lowercase()
            return when {
                ext == "pdf" -> Pdf
                ext == "svg" -> Svg
                ext in images -> Image
                ext == "md" || ext == "markdown" || ext == "mdx" -> Markdown
                ext == "html" || ext == "htm" || ext == "xhtml" -> Html
                ext in binaries -> Binary
                else -> Text
            }
        }
    }
}
