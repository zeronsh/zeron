package sh.zeron.android.update

import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import sh.zeron.android.core.UpdateDownloader
import sh.zeron.android.core.UpdateDownloader.Kind
import sh.zeron.android.core.UpdateSources.Source
import java.io.File
import java.util.Collections
import kotlin.random.Random

/** The multi-source downloader against a local HTTP server. */
class UpdateDownloaderTest {
    @get:Rule val tmp = TemporaryFolder()

    private val apk = Random(7).nextBytes(300_000)
    private val sha = UpdateDownloader.sha256Of(File.createTempFile("apk", null).apply { writeBytes(apk); deleteOnExit() })
    private lateinit var server: TinyHttpServer
    private val ranges = Collections.synchronizedList(mutableListOf<String>())
    private val auth = Collections.synchronizedList(mutableListOf<String>())

    @Before fun start() {
        server = TinyHttpServer()
        server.route("/good") { q, r -> serve(q, r, apk) }
        server.route("/broken") { _, r -> r.head(502, 0) }
        server.route("/wrong") { q, r -> serve(q, r, apk.copyOf().also { b -> b[1000] = (b[1000] + 1).toByte() }) }
        server.route("/html") { _, r ->
            val page = "<html>blocked</html>".toByteArray()
            r.head(200, page.size.toLong()); r.body(page)
        }
        // Sends the first 100 KB, then drops the connection.
        server.route("/half") { _, r -> r.head(200, apk.size.toLong()); r.body(apk, 0, 100_000) }
        server.route("/hang") { _, _ -> Thread.sleep(3_000) }
        server.route("/trickle") { _, r ->
            r.head(200, apk.size.toLong())
            for (i in apk.indices step 1_000) { r.body(apk, i, minOf(1_000, apk.size - i)); Thread.sleep(20) }
        }
        server.route("/redirect") { q, r ->
            auth += q.header("Authorization").orEmpty()
            r.head(302, 0, mapOf("Location" to "/cdn"))
        }
        server.route("/cdn") { q, r -> auth += q.header("Authorization").orEmpty(); serve(q, r, apk) }
        server.start()
    }

    @After fun stop() = server.stop()

    private fun serve(q: TinyHttpServer.Request, r: TinyHttpServer.Response, body: ByteArray) {
        val range = q.header("Range")
        ranges += range.orEmpty()
        val from = range?.removePrefix("bytes=")?.substringBefore('-')?.toIntOrNull() ?: 0
        if (from >= body.size) { r.head(416, 0); return }
        val extra = if (from > 0) mapOf("Content-Range" to "bytes $from-${body.size - 1}/${body.size}") else emptyMap()
        r.head(if (from > 0) 206 else 200, (body.size - from).toLong(), extra)
        r.body(body, from, body.size - from)
    }

    private fun src(name: String, token: String? = null) =
        Source(name, "http://127.0.0.1:${server.port}/$name", "key-$name", token)

    private fun downloader(slowWindowMs: Long = 60_000, slowMinBytes: Long = 0) = UpdateDownloader(
        userAgent = "test", connectTimeoutMs = 2_000, readTimeoutMs = 800,
        slowWindowMs = slowWindowMs, slowMinBytes = slowMinBytes, retryDelayMs = 10,
    )

    private fun part() = File(tmp.root, "u.apk.part")

    @Test fun fallsThroughToTheNextSourceOnHttpErrors() {
        val p = part()
        val won = downloader().download(listOf(src("broken"), src("html"), src("good")), p, apk.size.toLong(), sha) {}
        assertEquals("good", won.label)
        assertArrayEquals(apk, p.readBytes())
    }

    @Test fun resumesPartialBytesFromAnotherSource() {
        val p = part()
        val won = downloader().download(listOf(src("half"), src("good")), p, apk.size.toLong(), sha) {}
        assertEquals("good", won.label)
        assertEquals(listOf("bytes=100000-"), ranges.toList())
        assertArrayEquals(apk, p.readBytes())
    }

    @Test fun stalledSourceTimesOut() {
        val p = part()
        val won = downloader().download(listOf(src("hang"), src("good")), p, apk.size.toLong(), sha) {}
        assertEquals("good", won.label)
    }

    @Test fun slowSourceIsAbandonedWhileOthersRemain() {
        val p = part()
        var sawTrickle = false
        val won = downloader(slowWindowMs = 150, slowMinBytes = 1_000_000)
            .download(listOf(src("trickle"), src("good")), p, apk.size.toLong(), sha) { if (it.source == "trickle") sawTrickle = true }
        assertEquals("good", won.label)
        assertTrue(sawTrickle)
        // The slow source's bytes were kept and resumed.
        assertTrue(ranges.single().startsWith("bytes="))
        assertArrayEquals(apk, p.readBytes())
    }

    @Test fun lastSourceIsNeverAbandonedForSpeed() {
        val p = part()
        val won = downloader(slowWindowMs = 150, slowMinBytes = 1_000_000).download(listOf(src("trickle")), p, apk.size.toLong(), sha) {}
        assertEquals("trickle", won.label)
        assertArrayEquals(apk, p.readBytes())
    }

    @Test fun checksumMismatchDiscardsTheFile() {
        val p = part()
        val won = downloader().download(listOf(src("wrong"), src("good")), p, apk.size.toLong(), sha) {}
        assertEquals("good", won.label)
        assertEquals(listOf("", ""), ranges.toList()) // the bad file was not resumed
        assertArrayEquals(apk, p.readBytes())
    }

    @Test fun reportsEverySourceWhenAllFail() {
        val p = part()
        try {
            downloader().download(listOf(src("broken"), src("hang"), src("wrong")), p, apk.size.toLong(), sha) {}
            fail("expected failure")
        } catch (e: UpdateDownloader.Failed) {
            assertEquals(listOf("broken" to Kind.HTTP, "hang" to Kind.TIMEOUT, "wrong" to Kind.DIGEST), e.failures.map { it.source to it.kind })
            assertEquals("502", e.failures[0].detail)
        }
        assertFalse(p.exists())
    }

    @Test fun tokenIsNotForwardedPastTheRedirect() {
        // The token is only attached for github.com hosts, so a local
        // "api" source never sends it; the redirect hop must not either.
        val p = part()
        downloader().download(listOf(src("redirect", token = "secret")), p, apk.size.toLong(), sha) {}
        assertEquals(listOf("", ""), auth.toList())
        assertArrayEquals(apk, p.readBytes())
    }

    @Test fun unknownSizeAndDigestStillDownload() {
        val p = part()
        val won = downloader().download(listOf(src("good")), p, 0, null) {}
        assertEquals("good", won.label)
        assertArrayEquals(apk, p.readBytes())
    }

    @Test fun alreadyCompletePartIsJustVerified() {
        val p = part().apply { writeBytes(apk) }
        downloader().download(listOf(src("good")), p, apk.size.toLong(), sha) {}
        assertNull(ranges.firstOrNull())
    }

    @Test fun cancelStopsMidTransferAndKeepsThePart() {
        val p = part()
        val stop = java.util.concurrent.atomic.AtomicBoolean(false)
        val d = downloader()
        try {
            d.download(listOf(src("trickle"), src("good")), p, apk.size.toLong(), sha, cancelled = { stop.get() }) {
                if (it.done >= 50_000) stop.set(true)
            }
            fail("expected Cancelled")
        } catch (e: UpdateDownloader.Cancelled) {
            // expected: and the next source was not tried
        }
        assertTrue(ranges.isEmpty())
        val kept = p.length()
        assertTrue("kept $kept", kept in 50_000L until apk.size.toLong())
        assertArrayEquals(apk.copyOf(kept.toInt()), p.readBytes())
        // Switching to another source picks up at the same byte.
        val won = downloader().download(listOf(src("good"), src("trickle")), p, apk.size.toLong(), sha) {}
        assertEquals("good", won.label)
        assertEquals(listOf("bytes=$kept-"), ranges.toList())
        assertArrayEquals(apk, p.readBytes())
    }

    @Test fun cancelledBeforeStartingTouchesNothing() {
        val p = part()
        try {
            downloader().download(listOf(src("good")), p, apk.size.toLong(), sha, cancelled = { true }) {}
            fail("expected Cancelled")
        } catch (e: UpdateDownloader.Cancelled) {
        }
        assertTrue(ranges.isEmpty())
        assertFalse(p.exists())
    }

    @Test fun anotherSourceTakesOverWhileTheStoppedOneIsStillBlocked() {
        // Sends 10 KB, then stalls: the read blocks (plain JVM HttpURLConnection
        // doesn't unblock on disconnect), yet switching must not wait for it.
        server.route("/stall") { _, r -> r.head(200, apk.size.toLong()); r.body(apk, 0, 10_000); Thread.sleep(2_000) }
        val p = part()
        val d = UpdateDownloader(userAgent = "test", connectTimeoutMs = 2_000, readTimeoutMs = 10_000, retryDelayMs = 10)
        val oldStop = java.util.concurrent.atomic.AtomicBoolean(false)
        var oldResult: Throwable? = null
        val old = Thread {
            oldResult = runCatching { d.download(listOf(src("stall")), p, apk.size.toLong(), sha, cancelled = { oldStop.get() }) {} }.exceptionOrNull()
        }.apply { start() }
        val deadline = System.currentTimeMillis() + 3_000
        while (p.length() < 10_000 && System.currentTimeMillis() < deadline) Thread.sleep(10)
        assertEquals(10_000L, p.length())
        oldStop.set(true)
        d.abort()
        val started = System.nanoTime()
        val won = d.download(listOf(src("good")), p, apk.size.toLong(), sha) {}
        val tookMs = (System.nanoTime() - started) / 1_000_000
        assertEquals("good", won.label)
        assertTrue("took $tookMs ms", tookMs < 1_500)
        assertEquals(listOf("bytes=10000-"), ranges.toList())
        old.join(5_000)
        assertTrue("old run ended with $oldResult", oldResult is UpdateDownloader.Cancelled)
        // The stopped run never wrote after the switch.
        assertArrayEquals(apk, p.readBytes())
    }

    @Test fun eachGivenUpSourceIsReportedAsItHappens() {
        val seen = mutableListOf<Pair<String, Kind>>()
        downloader().download(listOf(src("broken"), src("html"), src("good")), part(), apk.size.toLong(), sha, onFailure = { seen += it.key to it.kind }) {}
        assertEquals(listOf("key-broken" to Kind.HTTP, "key-html" to Kind.HTTP), seen)
    }
}
