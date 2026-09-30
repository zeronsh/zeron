package sh.zeron.runtime

import android.system.ErrnoException
import android.system.Os
import android.system.OsConstants
import java.io.File
import java.io.FileOutputStream
import java.io.FilterInputStream
import java.io.IOException
import java.io.InputStream

/**
 * Minimal tar reader for the rootfs (ustar, GNU long names, pax paths). The
 * JDK has no tar, and the guest's own busybox tar can't run before a rootfs
 * exists. Symlinks are kept verbatim — absolute targets are guest paths that
 * proot resolves inside the rootfs. Hard links become copies: SELinux denies
 * link(2) in app data.
 */
internal class TarExtractor(private val dest: File) {
    private val header = ByteArray(BLOCK)

    fun extract(input: InputStream) {
        dest.mkdirs()
        val dirModes = ArrayList<Pair<File, Int>>()
        var longName: String? = null
        var longLink: String? = null
        var pax: Map<String, String> = emptyMap()

        while (readBlock(input, header)) {
            if (header.all { it.toInt() == 0 }) break
            val type = header[156].toInt().toChar()
            var size = parseNumber(124, 12)
            pax["size"]?.toLongOrNull()?.let { size = it }

            when (type) {
                'L' -> { longName = readString(input, size); continue }
                'K' -> { longLink = readString(input, size); continue }
                'x' -> { pax = parsePax(readString(input, size)); continue }
                'g' -> { skip(input, padded(size)); continue }
            }

            val name = pax["path"] ?: longName ?: ustarName()
            val link = pax["linkpath"] ?: longLink ?: cString(157, 100)
            val mode = parseNumber(100, 8).toInt()
            val mtime = parseNumber(136, 12)
            longName = null; longLink = null; pax = emptyMap()

            val target = resolve(name)
            if (target == null) {
                skip(input, padded(size))
                continue
            }
            when (type) {
                '5' -> {
                    if (!isRealDir(target)) { removeEntry(target); target.mkdirs() }
                    dirModes += target to mode
                }
                '2' -> {
                    removeEntry(target)
                    Os.symlink(link, target.path)
                }
                '1' -> {
                    val source = resolve(link)
                        ?: throw IOException("hard link $name → $link escapes the rootfs")
                    removeEntry(target)
                    source.copyTo(target)
                    chmod(target, mode or 0x180)
                }
                '0', '7', '\u0000' -> {
                    removeEntry(target)
                    FileOutputStream(target).use { out -> copy(input, out, size) }
                    skip(input, padded(size) - size)
                    chmod(target, (mode and 0x1ff) or 0x180) // at least u+rw
                    target.setLastModified(mtime * 1000)
                    continue
                }
                // Device nodes and fifos: the guest sees the host's /dev.
                else -> {}
            }
            skip(input, padded(size))
        }
        // Directories last, deepest first, so read-only ones don't block
        // their own contents; u+rwx keeps the tree deletable by reset().
        for ((dir, mode) in dirModes.asReversed()) chmod(dir, (mode and 0x1ff) or 0x1c0)
    }

    /** Maps an archive path into [dest], refusing `..` and symlinked parents. */
    private fun resolve(name: String): File? {
        val parts = name.split('/').filter { it.isNotEmpty() && it != "." }
        if (parts.isEmpty() || parts.any { it == ".." }) return null
        var dir = dest
        for (part in parts.dropLast(1)) {
            dir = File(dir, part)
            val st = lstat(dir)
            when {
                st == null -> dir.mkdir()
                OsConstants.S_ISLNK(st.st_mode) -> return null
                !OsConstants.S_ISDIR(st.st_mode) -> return null
            }
        }
        return File(dir, parts.last())
    }

    private fun isRealDir(file: File) = lstat(file)?.let { OsConstants.S_ISDIR(it.st_mode) } == true

    private fun removeEntry(file: File) {
        val st = lstat(file) ?: return
        if (OsConstants.S_ISDIR(st.st_mode)) deleteTree(file) else file.delete()
    }

    private fun ustarName(): String {
        val name = cString(0, 100)
        val magic = String(header, 257, 5, Charsets.US_ASCII)
        val prefix = if (magic == "ustar") cString(345, 155) else ""
        return if (prefix.isEmpty()) name else "$prefix/$name"
    }

    private fun cString(offset: Int, length: Int): String {
        var end = offset
        while (end < offset + length && header[end].toInt() != 0) end++
        return String(header, offset, end - offset, Charsets.UTF_8)
    }

    // Octal, or GNU base-256 when the high bit of the first byte is set.
    private fun parseNumber(offset: Int, length: Int): Long {
        if (header[offset].toInt() and 0x80 != 0) {
            var value = (header[offset].toLong() and 0x7f)
            for (i in offset + 1 until offset + length) value = (value shl 8) or (header[i].toLong() and 0xff)
            return value
        }
        val text = cString(offset, length).trim()
        return if (text.isEmpty()) 0 else text.toLong(8)
    }

    // pax records: "<len> key=value\n"
    private fun parsePax(text: String): Map<String, String> = text.lineSequence()
        .mapNotNull { line -> line.substringAfter(' ', "").split('=', limit = 2).takeIf { it.size == 2 } }
        .associate { it[0] to it[1] }

    private fun readString(input: InputStream, size: Long): String {
        val bytes = ByteArray(size.toInt())
        readFully(input, bytes)
        skip(input, padded(size) - size)
        return String(bytes, Charsets.UTF_8).trimEnd('\u0000', '\n')
    }

    private fun copy(input: InputStream, out: FileOutputStream, size: Long) {
        val buf = ByteArray(64 * 1024)
        var left = size
        while (left > 0) {
            val n = input.read(buf, 0, minOf(buf.size.toLong(), left).toInt())
            if (n < 0) throw IOException("truncated tar entry")
            out.write(buf, 0, n)
            left -= n
        }
    }

    private fun readBlock(input: InputStream, buf: ByteArray): Boolean {
        var off = 0
        while (off < buf.size) {
            val n = input.read(buf, off, buf.size - off)
            if (n < 0) {
                if (off == 0) return false
                throw IOException("truncated tar header")
            }
            off += n
        }
        return true
    }

    private fun readFully(input: InputStream, buf: ByteArray) {
        if (!readBlock(input, buf) && buf.isNotEmpty()) throw IOException("truncated tar entry")
    }

    private fun skip(input: InputStream, count: Long) {
        var left = count
        val buf = ByteArray(8192)
        while (left > 0) {
            val n = input.read(buf, 0, minOf(buf.size.toLong(), left).toInt())
            if (n < 0) throw IOException("truncated tar entry")
            left -= n
        }
    }

    private fun padded(size: Long) = (size + BLOCK - 1) / BLOCK * BLOCK

    private fun chmod(file: File, mode: Int) {
        try {
            Os.chmod(file.path, mode)
        } catch (_: ErrnoException) {
        }
    }

    companion object {
        private const val BLOCK = 512
    }
}

/** Counts bytes pulled through, for extraction progress. */
internal class CountingInputStream(input: InputStream) : FilterInputStream(input) {
    @Volatile var count = 0L
        private set

    override fun read(): Int = super.read().also { if (it >= 0) count++ }

    override fun read(b: ByteArray, off: Int, len: Int): Int =
        super.read(b, off, len).also { if (it > 0) count += it }

    override fun skip(n: Long): Long = super.skip(n).also { count += it }
}

internal fun lstat(file: File) = try {
    Os.lstat(file.path)
} catch (_: ErrnoException) {
    null
}

/**
 * rm -rf that never follows symlinks: absolute guest symlinks resolve to host
 * paths (/bin → /system/bin) and must not be descended into.
 */
internal fun deleteTree(file: File) {
    val st = lstat(file) ?: return
    if (OsConstants.S_ISDIR(st.st_mode)) {
        try {
            Os.chmod(file.path, 0x1c0)
        } catch (_: ErrnoException) {
        }
        file.list()?.forEach { deleteTree(File(file, it)) }
    }
    file.delete()
}
