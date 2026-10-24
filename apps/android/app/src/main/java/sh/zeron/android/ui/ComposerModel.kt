package sh.zeron.android.ui

import android.content.Context
import android.graphics.Bitmap
import android.graphics.ImageDecoder
import android.net.Uri
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

/** An image staged in the composer before sending (JPEG, long side ≤ 2560px). */
class StagedImage(val id: String, val name: String, val bytes: ByteArray, val thumb: ImageBitmap) {
    val outgoing get() = OutgoingAttachment(name, "image/jpeg", bytes)
}

/**
 * What's being composed: the prompt (with its cursor, for `@` mentions),
 * staged images, and the mention tokens it shows.
 */
@Stable
class ComposerModel(initial: String = "") {
    var value by mutableStateOf(TextFieldValue(initial, TextRange(initial.length)))
    val images = mutableStateListOf<StagedImage>()
    /** `@name` shown in the composer → the file it stands for. */
    private val tokens = LinkedHashMap<String, FileMatch>()

    val text: String get() = value.text
    val hasContent: Boolean get() = value.text.isNotBlank() || images.isNotEmpty()

    fun setText(s: String) {
        value = TextFieldValue(s, TextRange(s.length))
    }

    fun clear() {
        setText("")
        images.clear()
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
    const val MAX_IMAGES = 8
    private const val MAX_SIDE = 2560
    private const val MAX_BYTES = 24 * 1024 * 1024

    /** Decode, downscale and JPEG-encode a picked image (off the main thread). */
    suspend fun stage(context: Context, uri: Uri): StagedImage? = withContext(Dispatchers.IO) {
        runCatching {
            val source = ImageDecoder.createSource(context.contentResolver, uri)
            val bitmap = ImageDecoder.decodeBitmap(source) { decoder, info, _ ->
                val w = info.size.width
                val h = info.size.height
                val scale = min(1f, MAX_SIDE.toFloat() / max(w, h))
                if (scale < 1f) decoder.setTargetSize((w * scale).roundToInt(), (h * scale).roundToInt())
                decoder.allocator = ImageDecoder.ALLOCATOR_SOFTWARE
            }
            stage(bitmap)
        }.getOrNull()
    }

    fun stage(bitmap: Bitmap): StagedImage? {
        val out = ByteArrayOutputStream()
        bitmap.compress(Bitmap.CompressFormat.JPEG, 86, out)
        val bytes = out.toByteArray()
        if (bytes.size > MAX_BYTES) return null
        val t = 180f / max(bitmap.width, bitmap.height)
        val thumb = if (t < 1f) Bitmap.createScaledBitmap(bitmap, (bitmap.width * t).roundToInt(), (bitmap.height * t).roundToInt(), true) else bitmap
        val id = UUID.randomUUID().toString()
        return StagedImage(id, "image-${id.take(8)}.jpg", bytes, thumb.asImageBitmap())
    }
}
