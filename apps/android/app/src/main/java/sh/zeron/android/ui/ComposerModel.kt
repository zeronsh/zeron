package sh.zeron.android.ui

import android.content.Context
import android.graphics.Bitmap
import android.graphics.ImageDecoder
import android.net.Uri
import android.provider.OpenableColumns
import android.webkit.MimeTypeMap
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.input.TextFieldValue
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import uniffi.zeron_core.FileMatch
import uniffi.zeron_core.OutgoingAttachment
import uniffi.zeron_core.fileMentionLink
import java.io.ByteArrayOutputStream
import java.util.UUID
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt

/**
 * A file staged in the composer before sending. Pictures are downscaled JPEGs
 * with a [thumb]; anything else travels as the picked bytes, untouched.
 */
class StagedAttachment(val id: String, val name: String, val mime: String, val bytes: ByteArray, val thumb: ImageBitmap?) {
    val outgoing get() = OutgoingAttachment(name, mime, bytes)
}

/** Why a picked file was not staged, in words for a toast. */
class StagingException(message: String) : Exception(message)

/**
 * What's being composed: the prompt (with its cursor, for `@` mentions),
 * staged attachments, and the mention tokens it shows.
 */
@Stable
class ComposerModel(initial: String = "") {
    var value by mutableStateOf(TextFieldValue(initial, TextRange(initial.length)))
    val attachments = mutableStateListOf<StagedAttachment>()
    /** `@name` shown in the composer → the file it stands for. */
    private val tokens = LinkedHashMap<String, FileMatch>()

    val text: String get() = value.text
    val hasContent: Boolean get() = value.text.isNotBlank() || attachments.isNotEmpty()

    fun setText(s: String) {
        value = TextFieldValue(s, TextRange(s.length))
    }

    fun clear() {
        setText("")
        attachments.clear()
        tokens.clear()
    }

    /** The `@query` being typed at the cursor: its start offset and query. */
    fun activeQuery(): Pair<Int, String>? {
        val t = value.text
        if (!value.selection.collapsed) return null
        val cursor = value.selection.start
        var i = cursor - 1
        while (i >= 0) {
            val c = t[i]
            if (c == '@') {
                val before = if (i > 0) t[i - 1] else ' '
                return if (before == ' ' || before == '\n') i to t.substring(i + 1, cursor) else null
            }
            if (c == ' ' || c == '\n') return null
            i--
        }
        return null
    }

    /** Replace the `@query` with a token for `file` (parent folder on basename clashes). */
    fun insertMention(file: FileMatch) {
        val (start, _) = activeQuery() ?: return
        val parts = file.path.trim('/').split('/')
        var token = "@" + (parts.lastOrNull() ?: file.path)
        val existing = tokens[token]
        if (existing != null && existing.path != file.path && parts.size > 1) token = "@" + parts.takeLast(2).joinToString("/")
        tokens[token] = file
        val t = value.text
        val cursor = value.selection.start
        val next = t.substring(0, start) + token + " " + t.substring(cursor)
        value = TextFieldValue(next, TextRange(start + token.length + 1))
    }

    /** The text to send: live tokens become canonical `zeron-file:` links. */
    fun encoded(): String {
        var out = value.text
        for ((token, file) in tokens.entries.sortedByDescending { it.key.length }) {
            if (out.contains(token)) out = out.replace(token, fileMentionLink(file.path, file.isDir))
        }
        return out.trim()
    }
}

object Staging {
    const val MAX_ATTACHMENTS = 8
    private const val MAX_SIDE = 2560
    /** Per file; the shared client refuses anything larger. */
    const val MAX_BYTES = 24 * 1024 * 1024
    /** Everything staged at once lives in memory until it is sent. */
    const val MAX_TOTAL_BYTES = 64 * 1024 * 1024

    /**
     * Read a picked file (off the main thread). Pictures are downscaled to
     * JPEG; every other type is kept byte for byte. Throws [StagingException]
     * with a reason the user can read.
     */
    suspend fun stage(context: Context, uri: Uri): StagedAttachment = withContext(Dispatchers.IO) {
        val resolver = context.contentResolver
        var name: String? = null
        var size = -1L
        runCatching {
            resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)?.use { c ->
                if (c.moveToFirst()) {
                    name = c.getString(0)
                    if (!c.isNull(1)) size = c.getLong(1)
                }
            }
        }
        val label = name?.takeIf { it.isNotBlank() } ?: uri.lastPathSegment?.substringAfterLast('/') ?: "file"
        val mime = resolver.getType(uri)?.takeIf { it != "application/octet-stream" }
            ?: MimeTypeMap.getSingleton().getMimeTypeFromExtension(label.substringAfterLast('.', "").lowercase())
            ?: "application/octet-stream"
        if (size > MAX_BYTES) throw StagingException("$label is larger than ${MAX_BYTES / 1024 / 1024} MB")
        // Animated and vector images keep their bytes: JPEG would flatten them.
        if (mime.startsWith("image/") && mime != "image/gif" && mime != "image/svg+xml") {
            val image = runCatching { stageImage(context, uri, label) }.getOrNull()
            if (image != null) return@withContext image
        }
        val bytes = try {
            resolver.openInputStream(uri)?.use { it.readBounded(MAX_BYTES) }
        } catch (e: java.io.IOException) {
            null
        } ?: throw StagingException("Couldn't read $label")
        if (bytes.size > MAX_BYTES) throw StagingException("$label is larger than ${MAX_BYTES / 1024 / 1024} MB")
        StagedAttachment(UUID.randomUUID().toString(), label, mime, bytes, null)
    }

    private fun stageImage(context: Context, uri: Uri, label: String): StagedAttachment? {
        val source = ImageDecoder.createSource(context.contentResolver, uri)
        val bitmap = ImageDecoder.decodeBitmap(source) { decoder, info, _ ->
            val w = info.size.width
            val h = info.size.height
            val scale = min(1f, MAX_SIDE.toFloat() / max(w, h))
            if (scale < 1f) decoder.setTargetSize((w * scale).roundToInt(), (h * scale).roundToInt())
            decoder.allocator = ImageDecoder.ALLOCATOR_SOFTWARE
        }
        return stage(bitmap, label.substringBeforeLast('.').ifBlank { null }?.let { "$it.jpg" })
    }

    fun stage(bitmap: Bitmap, fileName: String? = null): StagedAttachment? {
        val out = ByteArrayOutputStream()
        bitmap.compress(Bitmap.CompressFormat.JPEG, 86, out)
        val bytes = out.toByteArray()
        if (bytes.size > MAX_BYTES) return null
        val t = 180f / max(bitmap.width, bitmap.height)
        val thumb = if (t < 1f) Bitmap.createScaledBitmap(bitmap, (bitmap.width * t).roundToInt(), (bitmap.height * t).roundToInt(), true) else bitmap
        val id = UUID.randomUUID().toString()
        return StagedAttachment(id, fileName ?: "image-${id.take(8)}.jpg", "image/jpeg", bytes, thumb.asImageBitmap())
    }

    /** Reads at most [limit] + 1 bytes so an oversized stream of unknown length is noticed, not slurped. */
    private fun java.io.InputStream.readBounded(limit: Int): ByteArray {
        val out = ByteArrayOutputStream()
        val buffer = ByteArray(64 * 1024)
        while (out.size() <= limit) {
            val n = read(buffer, 0, minOf(buffer.size, limit + 1 - out.size()))
            if (n < 0) break
            out.write(buffer, 0, n)
        }
        return out.toByteArray()
    }
}
