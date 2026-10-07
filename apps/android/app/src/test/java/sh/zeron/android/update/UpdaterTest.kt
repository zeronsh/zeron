package sh.zeron.android.update

import android.app.Application
import androidx.test.core.app.ApplicationProvider
import kotlinx.coroutines.runBlocking
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import sh.zeron.android.core.UpdateDownloader
import sh.zeron.android.core.Updater
import java.io.File
import java.util.Collections
import kotlin.random.Random

/** The update check's fallback chain and the download wiring, against a local server. */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34])
class UpdaterTest {
    private lateinit var server: TinyHttpServer
    private val hits = Collections.synchronizedList(mutableListOf<String>())
    private val apk = Random(3).nextBytes(120_000)
    private val sha = UpdateDownloader.sha256Of(File.createTempFile("apk", null).apply { writeBytes(apk); deleteOnExit() })
    private val repo = "/repos/villatothesea/zeron-android-app/releases/latest"

    private val api get() = "http://127.0.0.1:${server.port}/api"
    private val web get() = "http://localhost:${server.port}/web"
    private val mirror get() = "http://127.0.0.1:${server.port}/m/"

    @Before fun start() {
        server = TinyHttpServer().start()
    }

    @After fun stop() = server.stop()

    private fun route(path: String, handler: (TinyHttpServer.Request, TinyHttpServer.Response) -> Unit) =
        server.route(path) { q, r -> hits += q.path; handler(q, r) }

    private fun status(code: Int): (TinyHttpServer.Request, TinyHttpServer.Response) -> Unit = { _, r -> r.head(code, 0) }

    private fun json(tag: String, url: String): String = """
        {"tag_name":"$tag","name":"$tag","html_url":"https://github.com/x/releases/tag/$tag",
         "body":"notes\r\n\r\nversionCode: 777",
         "assets":[{"name":"zeron-android-$tag.apk","size":${apk.size},"url":"","browser_download_url":"$url",
                    "digest":"sha256:$sha"}]}
    """.trimIndent()

    private fun updater(check: Updater.Signature = Updater.Signature.MATCH) = Updater(
        ApplicationProvider.getApplicationContext<Application>(), api = api, web = web,
        builtInMirrors = listOf(mirror), signatureCheck = { check },
    )

    @Test fun rateLimitedApiFallsBackToTheWebRedirect() = runBlocking {
        route("/api", status(403))
        route("/web") { _, r -> r.head(302, 0, mapOf("Location" to "https://github.com/villatothesea/zeron-android-app/releases/tag/round9-1")) }
        val rel = updater().latest()
        assertEquals("round9-1", rel.tag)
        assertEquals(901L, rel.versionCode)
        assertNull(rel.sha256)
        assertEquals("$web/villatothesea/zeron-android-app/releases/download/round9-1/zeron-android-round9-1.apk", rel.downloadUrl)
    }

    @Test fun unreachableGithubFallsBackToAMirror() = runBlocking {
        route("/api", status(500))
        route("/web", status(502))
        route("/m/$api$repo") { _, r -> val b = json("round9-2", "https://github.com/dl.apk").toByteArray(); r.head(200, b.size.toLong()); r.body(b) }
        val rel = updater().latest()
        assertEquals("round9-2", rel.tag)
        assertEquals(777L, rel.versionCode)
        assertEquals(sha, rel.sha256)
        assertEquals(apk.size.toLong(), rel.size)
        assertEquals("notes", rel.notes)
    }

    @Test fun noReleaseStopsWithoutAskingMirrors() = runBlocking {
        route("/api", status(404))
        route("/m/", status(500))
        try {
            updater().latest(); fail("expected error")
        } catch (e: Updater.UpdateError) {
            assertEquals("No release published yet.", e.message)
        }
        assertTrue(hits.none { it.startsWith("/m/") })
    }

    @Test fun everySourceIsListedWhenAllFail() = runBlocking {
        route("/api", status(403))
        route("/web", status(500))
        route("/m/", status(503))
        try {
            updater().latest(); fail("expected error")
        } catch (e: Updater.UpdateError) {
            val lines = e.message!!.lines()
            assertEquals("Couldn't reach GitHub or any mirror:", lines[0])
            assertEquals("• 127.0.0.1: rate limited (HTTP 403)", lines[1])
            assertEquals("• localhost: HTTP 500", lines[2])
            assertEquals("• 127.0.0.1: HTTP 503", lines[3]) // mirror API
            assertEquals("• 127.0.0.1: HTTP 503", lines[4]) // mirror redirect
        }
    }

    @Test fun downloadFallsBackToTheMirrorAndRemembersIt() = runBlocking {
        route("/web/dl.apk", status(502))
        route("/m/") { _, r -> r.head(200, apk.size.toLong()); r.body(apk) }
        val u = updater()
        val rel = Updater.Release("round9-3", "round9-3", "", 903, "zeron-android-round9-3.apk", "", "$web/dl.apk", apk.size.toLong(), "", sha)
        val sources = mutableSetOf<String>()
        val file = u.download(rel) { sources += it.source }
        assertArrayEquals(apk, file.readBytes())
        assertEquals(setOf("127.0.0.1"), sources)
        assertEquals(mirror, u.preferredSource)
        assertEquals(mirror + rel.downloadUrl, u.browserUrl(rel))
    }

    @Test fun downloadErrorNamesEachSource() = runBlocking {
        route("/web/dl.apk", status(502))
        route("/m/", status(404))
        val rel = Updater.Release("round9-4", "round9-4", "", 904, "a.apk", "", "$web/dl.apk", apk.size.toLong(), "", sha)
        try {
            updater().download(rel) {}; fail("expected error")
        } catch (e: Updater.UpdateError) {
            assertEquals(listOf("Download failed from every source:", "• GitHub: HTTP 502", "• 127.0.0.1: HTTP 404"), e.message!!.lines())
        }
    }

    @Test fun wronglySignedApkIsRejected() = runBlocking {
        route("/web/dl.apk") { _, r -> r.head(200, apk.size.toLong()); r.body(apk) }
        val rel = Updater.Release("round9-5", "round9-5", "", 905, "a.apk", "", "$web/dl.apk", apk.size.toLong(), "", sha)
        try {
            updater(check = Updater.Signature.MISMATCH).download(rel) {}; fail("expected error")
        } catch (e: Updater.UpdateError) {
            assertEquals("The file from GitHub isn't signed with the Zeron release key, so it won't be installed.", e.message)
        }
    }

    @Test fun foundReleaseIsRememberedAcrossRestarts() = runBlocking {
        route("/api$repo") { _, r -> val b = json("round9-6", "https://github.com/dl.apk").toByteArray(); r.head(200, b.size.toLong()); r.body(b) }
        val found = updater().latest()
        // A fresh Updater (app restart) reads it back without a network call.
        assertEquals(found, updater().lastKnown())
    }

    @Test fun cachedApkIsPickedUpAndStaleOnesAreDropped() = runBlocking {
        route("/web/dl.apk") { _, r -> r.head(200, apk.size.toLong()); r.body(apk) }
        val u = updater()
        val rel = Updater.Release("round9-7", "round9-7", "", 90_700, "a.apk", "", "$web/dl.apk", apk.size.toLong(), "", sha)
        assertNull(u.cached(rel))
        val file = u.download(rel) {}
        assertEquals(file, u.cached(rel))
        // A leftover from an older release goes; the newer one stays.
        val old = File(file.parentFile, "round1-1.apk").apply { writeBytes(byteArrayOf(1)) }
        u.cleanStale(rel)
        assertTrue(file.exists())
        assertTrue(!old.exists())
        // Nothing newer than the running build: everything goes.
        u.cleanStale(null)
        assertTrue(!file.exists())
        assertNull(u.cached(rel))
    }

    @Test fun autoUpdateDefaultsOn() {
        val u = updater()
        assertTrue(u.autoUpdate)
        u.autoUpdate = false
        assertTrue(!updater().autoUpdate)
    }

    @Test fun cancelledDownloadKeepsItsPartAndAnotherSourceResumesIt() = runBlocking {
        val ranges = Collections.synchronizedList(mutableListOf<String>())
        route("/web/dl.apk") { _, r ->
            r.head(200, apk.size.toLong())
            for (i in apk.indices step 1_000) { r.body(apk, i, minOf(1_000, apk.size - i)); Thread.sleep(15) }
        }
        route("/m/") { q, r ->
            val from = q.header("Range")?.also { ranges += it }?.removePrefix("bytes=")?.substringBefore('-')?.toInt() ?: 0
            val extra = if (from > 0) mapOf("Content-Range" to "bytes $from-${apk.size - 1}/${apk.size}") else emptyMap()
            r.head(if (from > 0) 206 else 200, (apk.size - from).toLong(), extra)
            r.body(apk, from, apk.size - from)
        }
        val u = updater()
        val rel = Updater.Release("round9-8", "round9-8", "", 908, "a.apk", "", "$web/dl.apk", apk.size.toLong(), "", sha)
        val stop = java.util.concurrent.atomic.AtomicBoolean(false)
        try {
            u.download(rel, cancelled = { stop.get() }) { if (it.done >= 30_000) stop.set(true) }
            fail("expected Cancelled")
        } catch (e: UpdateDownloader.Cancelled) {
        }
        val kept = u.partialBytes(rel)
        assertTrue("kept $kept", kept in 30_000L until apk.size.toLong())
        assertEquals(listOf("GitHub", "127.0.0.1"), u.sources(rel).map { it.label })
        // 换个镜像 (Switch mirror) -> the mirror, from the same byte.
        val file = u.download(rel, prefer = mirror) {}
        assertEquals(listOf("bytes=$kept-"), ranges.toList())
        assertArrayEquals(apk, file.readBytes())
        assertEquals(mirror, u.preferredSource)
        assertEquals(0L, u.partialBytes(rel))
    }

    @Test fun discardingThePartStartsOver() = runBlocking {
        val u = updater()
        val rel = Updater.Release("round9-9", "round9-9", "", 909, "a.apk", "", "$web/dl.apk", apk.size.toLong(), "", sha)
        val app = ApplicationProvider.getApplicationContext<Application>()
        File(app.cacheDir, "updates").mkdirs()
        File(app.cacheDir, "updates/round9-9.apk.part").writeBytes(apk.copyOf(4_000))
        assertEquals(4_000L, u.partialBytes(rel))
        u.discardPartial(rel)
        assertEquals(0L, u.partialBytes(rel))
    }
}
