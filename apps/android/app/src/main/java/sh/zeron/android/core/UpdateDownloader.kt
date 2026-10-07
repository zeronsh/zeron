package sh.zeron.android.core

import java.io.File
import java.io.IOException
import java.io.InterruptedIOException
import java.io.RandomAccessFile
import java.net.HttpURLConnection
import java.net.SocketTimeoutException
import java.net.URL
import java.security.MessageDigest

/**
 * Resumable APK download that walks a list of [UpdateSources.Source]s.
 *
 * A source is abandoned (and the next one tried, keeping the bytes already on
 * disk) when it fails to connect, stops sending for [readTimeoutMs], answers
 * with an error, or crawls below [slowMinBytes] per [slowWindowMs] while other
 * sources are still untried. The finished file must match the expected size
 * and SHA-256 when known; a mismatch discards it and moves on.
 *
 * No Android dependencies, so it runs in plain JVM tests against a local server.
 */
class UpdateDownloader(
    private val userAgent: String,
    private val connectTimeoutMs: Int = 10_000,
    private val readTimeoutMs: Int = 20_000,
    private val slowWindowMs: Long = 30_000,
    private val slowMinBytes: Long = 256L * 1024,
    private val attemptsPerSource: Int = 2,
    private val retryDelayMs: Long = 1_500,
    private val nowMs: () -> Long = { System.nanoTime() / 1_000_000 },
) {
    data class Progress(val done: Long, val total: Long, val source: String, val bytesPerSec: Long, val key: String = "")

    enum class Kind { TIMEOUT, SLOW, HTTP, SIZE, DIGEST, DNS, TLS, IO }

    data class Failure(val source: String, val kind: Kind, val detail: String = "", val key: String = "")

    class Failed(val failures: List<Failure>) : IOException(failures.joinToString("; ") { "${it.source}: ${it.kind} ${it.detail}".trim() })

    private class SourceError(val kind: Kind, val detail: String = "") : IOException("$kind $detail")

    /** The caller stopped the download ([download]'s `cancelled`, or [abort]); the .part file stays. */
    class Cancelled : IOException("cancelled")

    @Volatile private var current: HttpURLConnection? = null

    /**
     * Guards the .part file: every write checks `cancelled` under it, so once
     * a run is cancelled it never writes again, even while its read is still
     * blocked. A new run (another source) can start on the same file at once.
     */
    private val fileLock = Any()

    /** Run [block] while no download is mid-write (to measure or delete the .part file). */
    fun <T> withFile(block: () -> T): T = synchronized(fileLock) { block() }

    /**
     * Drop the connection in use now, so a blocked read returns early (pair
     * with `cancelled`). Android's HttpURLConnection unblocks the read; where
     * it doesn't, the stopped run still never writes again (see [fileLock]).
     */
    fun abort() {
        runCatching { current?.disconnect() }
    }

    /**
     * Downloads into [part] and returns the source that finished it. [part]
     * is left in place on failure so a later call resumes it. [cancelled] is
     * polled between chunks and attempts; when it turns true the call throws
     * [Cancelled]. [onFailure] hears about each source given up on, as it happens.
     */
    fun download(
        sources: List<UpdateSources.Source>,
        part: File,
        expectedSize: Long,
        sha256: String?,
        cancelled: () -> Boolean = { false },
        onFailure: (Failure) -> Unit = {},
        progress: (Progress) -> Unit,
    ): UpdateSources.Source {
        val failures = mutableListOf<Failure>()
        sources.forEachIndexed { index, source ->
            val last = index == sources.lastIndex
            var attempt = 0
            while (true) {
                if (cancelled()) throw Cancelled()
                try {
                    fetch(source, part, expectedSize, allowSlowAbort = !last, cancelled, progress)
                    if (cancelled()) throw Cancelled()
                    verify(part, expectedSize, sha256)
                    return source
                } catch (e: Cancelled) {
                    throw e
                } catch (e: IOException) {
                    if (cancelled()) throw Cancelled()
                    if (!handle(e, source, part, failures, ++attempt)) {
                        onFailure(failures.last())
                        break
                    }
                    Thread.sleep(retryDelayMs)
                    continue
                }
            }
        }
        throw Failed(failures)
    }

    /** Retry the same source (true), or record a failure for [source] in [failures] (false). */
    private fun handle(e: IOException, source: UpdateSources.Source, part: File, failures: MutableList<Failure>, attempt: Int): Boolean {
        try {
            throw e
        } catch (e: SourceError) {
            if (e.kind == Kind.DIGEST || e.kind == Kind.SIZE) part.delete()
            // Timeouts, slowness and bad files: straight to the next source.
            if (e.kind != Kind.IO || attempt >= attemptsPerSource) {
                failures += Failure(source.label, e.kind, e.detail, key = source.key)
                return false
            }
            return true
        } catch (e: SocketTimeoutException) {
            failures += Failure(source.label, Kind.TIMEOUT, key = source.key)
        } catch (e: InterruptedIOException) {
            failures += Failure(source.label, Kind.TIMEOUT, key = source.key)
        } catch (e: java.net.UnknownHostException) {
            failures += Failure(source.label, Kind.DNS, key = source.key)
        } catch (e: javax.net.ssl.SSLException) {
            failures += Failure(source.label, Kind.TLS, e.message.orEmpty(), key = source.key)
        } catch (e: IOException) {
            if (attempt < attemptsPerSource) return true
            failures += Failure(source.label, Kind.IO, e.message ?: e.javaClass.simpleName, key = source.key)
        }
        return false
    }

    private fun verify(part: File, expectedSize: Long, sha256: String?) {
        if (expectedSize > 0 && part.length() != expectedSize) {
            throw SourceError(Kind.SIZE, "${part.length()} != $expectedSize")
        }
        if (sha256 != null) {
            val actual = sha256Of(part)
            if (!actual.equals(sha256, ignoreCase = true)) throw SourceError(Kind.DIGEST)
        }
    }

    private fun open(url: URL, token: String?): HttpURLConnection {
        val conn = url.openConnection() as HttpURLConnection
        conn.connectTimeout = connectTimeoutMs
        conn.readTimeout = readTimeoutMs
        conn.instanceFollowRedirects = false
        conn.setRequestProperty("User-Agent", userAgent)
        if (token != null) {
            conn.setRequestProperty("Authorization", "Bearer $token")
            conn.setRequestProperty("Accept", "application/octet-stream")
        }
        return conn
    }

    private fun fetch(
        source: UpdateSources.Source,
        part: File,
        total: Long,
        allowSlowAbort: Boolean,
        cancelled: () -> Boolean,
        progress: (Progress) -> Unit,
    ) {
        var url = URL(source.url)
        var token = source.token?.takeIf { url.host.endsWith("github.com") }
        var hops = 0
        while (true) {
            if (cancelled()) throw Cancelled()
            val have = withFile { if (part.exists()) part.length() else 0L }
            if (total > 0 && have >= total) return
            val conn = open(url, token)
            current = conn
            try {
                if (have > 0) conn.setRequestProperty("Range", "bytes=$have-")
                val code = conn.responseCode
                if (code in 300..399) {
                    val next = conn.getHeaderField("Location") ?: throw SourceError(Kind.HTTP, "$code without Location")
                    url = URL(url, next)
                    token = null // never send the token to the CDN
                    if (++hops > 6) throw SourceError(Kind.HTTP, "too many redirects")
                    continue
                }
                if (code == 416) {
                    // Nothing left to send for this offset: either complete, or a stale file.
                    if (total > 0 && have == total) return
                    part.delete()
                    continue
                }
                if (code !in 200..299) throw SourceError(Kind.HTTP, code.toString())
                val append = code == 206 && have > 0
                val base = if (append) have else 0L
                val length = conn.contentLengthLong.let { if (it > 0) it + base else total }
                // A proxy error page or a different file: leave the bytes on disk alone.
                if (total > 0 && length > 0 && length != total) throw SourceError(Kind.HTTP, "size $length != $total")
                copy(conn, part, append, base, length, source, allowSlowAbort, cancelled, progress)
                return
            } finally {
                if (current === conn) current = null
                conn.disconnect()
            }
        }
    }

    private fun copy(
        conn: HttpURLConnection,
        part: File,
        append: Boolean,
        base: Long,
        length: Long,
        source: UpdateSources.Source,
        allowSlowAbort: Boolean,
        cancelled: () -> Boolean,
        progress: (Progress) -> Unit,
    ) {
        val out = withFile {
            if (cancelled()) throw Cancelled()
            RandomAccessFile(part, "rw").also { if (!append) it.setLength(0) }
        }
        out.use { out ->
            out.seek(base)
            var written = base
            var windowStart = nowMs()
            var windowBytes = 0L
            var speedMark = windowStart
            var speedBytes = written
            var speed = 0L
            var lastReport = 0L
            val buf = ByteArray(64 * 1024)
            conn.inputStream.use { input ->
                while (true) {
                    val n = input.read(buf)
                    if (n < 0) break
                    withFile {
                        if (cancelled()) throw Cancelled()
                        out.write(buf, 0, n)
                    }
                    written += n
                    windowBytes += n
                    val now = nowMs()
                    if (now - speedMark >= 1_000) {
                        speed = (written - speedBytes) * 1_000 / (now - speedMark)
                        speedMark = now
                        speedBytes = written
                    }
                    if (now - windowStart >= slowWindowMs) {
                        if (allowSlowAbort && windowBytes < slowMinBytes) {
                            throw SourceError(Kind.SLOW, "${windowBytes * 1_000 / (now - windowStart)}")
                        }
                        windowStart = now
                        windowBytes = 0
                    }
                    if (now - lastReport >= 200 || (length > 0 && written >= length)) {
                        lastReport = now
                        progress(Progress(written, length, source.label, speed, source.key))
                    }
                }
            }
            if (length > 0 && written < length) throw IOException("connection closed early")
        }
    }

    companion object {
        fun sha256Of(file: File): String {
            val md = MessageDigest.getInstance("SHA-256")
            file.inputStream().use { input ->
                val buf = ByteArray(64 * 1024)
                while (true) {
                    val n = input.read(buf)
                    if (n < 0) break
                    md.update(buf, 0, n)
                }
            }
            return md.digest().joinToString("") { "%02x".format(it) }
        }
    }
}
