package sh.zeron.runtime

import android.util.Log
import java.io.File
import java.io.RandomAccessFile
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/**
 * Engine stdout/stderr plus the runtime's own notes, in engine.log rotated to
 * engine.log.1 at [maxBytes] so a chatty engine can't fill the phone.
 */
internal class RuntimeLog(private val dir: File, private val maxBytes: Long = 2L * 1024 * 1024) {
    private val current get() = File(dir, "engine.log")
    private val previous get() = File(dir, "engine.log.1")
    private val stamp = SimpleDateFormat("HH:mm:ss.SSS", Locale.US)

    /** A line from the runtime itself (not the engine). */
    fun note(message: String) {
        Log.i(TAG, message)
        append("${stamp.format(Date())} [runtime] $message")
    }

    @Synchronized
    fun append(line: String) {
        try {
            dir.mkdirs()
            val file = current
            if (file.length() > maxBytes) {
                previous.delete()
                file.renameTo(previous)
            }
            file.appendText(line.trimEnd('\n') + "\n")
        } catch (e: Exception) {
            Log.w(TAG, "log write failed", e)
        }
    }

    @Synchronized
    fun tail(lines: Int): String {
        if (lines <= 0) return ""
        val out = ArrayDeque<String>()
        for (file in listOf(current, previous)) {
            if (out.size >= lines || !file.exists()) continue
            val chunk = tailOf(file, lines - out.size)
            for (line in chunk.asReversed()) out.addFirst(line)
        }
        return out.joinToString("\n")
    }

    // Reads backwards in blocks; logs can be megabytes, tails are tiny.
    private fun tailOf(file: File, lines: Int): List<String> {
        RandomAccessFile(file, "r").use { raf ->
            var pos = raf.length()
            val block = 16 * 1024
            var bytes = ByteArray(0)
            while (pos > 0) {
                val size = minOf(block.toLong(), pos).toInt()
                pos -= size
                val chunk = ByteArray(size)
                raf.seek(pos)
                raf.readFully(chunk)
                bytes = chunk + bytes
                if (bytes.count { it == '\n'.code.toByte() } > lines) break
            }
            return String(bytes).trimEnd('\n').split('\n').takeLast(lines)
        }
    }

    @Synchronized
    fun clear() {
        current.delete()
        previous.delete()
    }

    companion object {
        const val TAG = "ZeronRuntime"
    }
}
