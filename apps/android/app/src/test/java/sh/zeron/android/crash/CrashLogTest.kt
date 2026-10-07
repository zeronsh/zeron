package sh.zeron.android.crash

import android.app.Application
import androidx.test.core.app.ApplicationProvider
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import sh.zeron.android.BuildConfig
import sh.zeron.android.core.CrashLog

/** The local crash log: report contents, scrubbing, rotation, the next-launch marker, handler chaining. */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34])
class CrashLogTest {
    private val app get() = ApplicationProvider.getApplicationContext<Application>()
    private val t0 = 1_790_000_000_000L
    private var saved: Thread.UncaughtExceptionHandler? = null

    @Before fun setUp() {
        saved = Thread.getDefaultUncaughtExceptionHandler()
        CrashLog.clear(app)
        CrashLog.extraNames = { emptyList() }
        app.getSharedPreferences("zeron-machines", 0).edit().clear().commit()
    }

    @After fun tearDown() {
        Thread.setDefaultUncaughtExceptionHandler(saved)
        CrashLog.extraNames = { emptyList() }
    }

    private fun boom(message: String): Throwable = try {
        throw IllegalStateException(message, java.io.IOException("inner cause"))
    } catch (e: IllegalStateException) { e }

    @Test fun reportCarriesBuildPhoneThreadAndFullTrace() {
        val text = CrashLog.report("main", boom("list index 3"), t0, android = "14 (API 34)", device = "Google Pixel 8")
        assertTrue(text.startsWith("Zeron crash log\n"))
        assertTrue(text.contains("Time: "))
        assertTrue(text.contains("App: ${BuildConfig.VERSION_NAME} (versionCode ${BuildConfig.VERSION_CODE})"))
        assertTrue(text.contains("Android: 14 (API 34)"))
        assertTrue(text.contains("Device: Google Pixel 8"))
        assertTrue(text.contains("Thread: main"))
        assertTrue(text.contains("java.lang.IllegalStateException: list index 3"))
        assertTrue(text.contains("at sh.zeron.android.crash.CrashLogTest.boom(CrashLogTest.kt:"))
        assertTrue(text.contains("Caused by: java.io.IOException: inner cause"))
    }

    @Test fun scrubsAddressesHostsAndUsers() {
        val raw = """
            java.net.ConnectException: failed to connect to /192.168.1.23 (port 22) from /10.0.0.7 (port 51234)
            ssh okhlv@studio-mac.local refused; tried [fe80::1c2b:3d4e:5f60:7a8b%wlan0]:27654 and 2001:db8:85a3::8a2e:370:7334
            GET https://alice:secret@edge.example.com:8443/v1/sync failed, mail bob.smith@example.org
            open /home/okhlv/projects/x and /Users/Alice/src and C:\Users\carol\app
            host mbp.tail1234.ts.net via NAS-Box
            	at sh.zeron.android.core.ZeronModel.start(ZeronModel.kt:971)
            	at java.lang.Object@1a2b3c4d.toString(Unknown Source:12)
            	at kotlinx.coroutines.DispatchedTask.run(DispatchedTask.kt:108)
            	at java.base/jdk.internal.reflect.DirectMethodHandleAccessor.invoke(DirectMethodHandleAccessor.java:103)
            Time 19:20:31, version 1.2.3.4.5
        """.trimIndent()
        val s = CrashLog.redact(raw, listOf("NAS-Box", "okhlv", "main"))
        for (leak in listOf("192.168.1.23", "10.0.0.7", "okhlv", "studio-mac", "fe80", "2001:db8", "alice", "secret", "edge.example.com",
                "bob.smith", "example.org", "Alice", "carol", "mbp.tail1234", "NAS-Box")) {
            assertFalse("leaked $leak in:\n$s", s.contains(leak))
        }
        assertTrue(s, s.contains("/<ip> (port 22)"))
        assertTrue(s, s.contains("https://<host>:8443/v1/sync"))
        assertTrue(s, s.contains("/home/<user>/projects/x"))
        // Stack frames, hashes, clock times and versions survive.
        assertTrue(s, s.contains("at sh.zeron.android.core.ZeronModel.start(ZeronModel.kt:971)"))
        assertTrue(s, s.contains("java.lang.Object@1a2b3c4d"))
        assertTrue(s, s.contains("kotlinx.coroutines.DispatchedTask.run(DispatchedTask.kt:108)"))
        assertTrue(s, s.contains("Time 19:20:31, version 1.2.3.4.5"))
        assertTrue(s, s.contains("at java.base/jdk.internal.reflect.DirectMethodHandleAccessor.invoke(DirectMethodHandleAccessor.java:103)"))
    }

    @Test fun writeScrubsSavedComputersAndWorkspaceNames() {
        app.getSharedPreferences("zeron-machines", 0).edit().putString("machines",
            """[{"id":"m1","name":"Okhlv Studio","host":"studio.home","user":"okhlv","endpoints":[{"host":"100.64.0.9","port":22}]}]""").commit()
        CrashLog.extraNames = { listOf("Build Box") }
        val f = CrashLog.write(app, "worker Okhlv Studio", IllegalArgumentException("sync with Build Box as okhlv on studio.home failed"), t0)
        val text = f.readText()
        for (leak in listOf("Okhlv Studio", "Build Box", "okhlv", "studio.home")) assertFalse("leaked $leak:\n$text", text.contains(leak))
        assertTrue(text, text.contains("Thread: worker <name>"))
        assertTrue(text, text.contains("sync with <name> as <name> on <name> failed"))
    }

    @Test fun keepsTheLastFiveNewestFirst() {
        for (i in 0 until 7) CrashLog.write(app, "main", RuntimeException("crash $i"), t0 + i * 1000)
        val list = CrashLog.list(app)
        assertEquals(CrashLog.KEEP, list.size)
        assertEquals((6 downTo 2).map { "java.lang.RuntimeException: crash $it" }, list.map { it.headline })
        assertEquals(t0 + 6000, list.first().atMs)
    }

    @Test fun pendingUntilSeenAndClear() {
        assertNull(CrashLog.pending(app))
        CrashLog.write(app, "main", RuntimeException("first"), t0)
        CrashLog.write(app, "main", RuntimeException("second"), t0 + 1)
        assertEquals("java.lang.RuntimeException: second", CrashLog.pending(app)?.headline)
        CrashLog.markSeen(app)
        assertNull(CrashLog.pending(app))
        assertEquals(2, CrashLog.list(app).size)
        CrashLog.write(app, "main", RuntimeException("third"), t0 + 2)
        assertNotNull(CrashLog.pending(app))
        CrashLog.clear(app)
        assertNull(CrashLog.pending(app))
        assertTrue(CrashLog.list(app).isEmpty())
    }

    @Test fun handlerWritesThenChainsToThePreviousOne() {
        val seen = mutableListOf<Throwable>()
        val previous = Thread.UncaughtExceptionHandler { _, e -> seen += e }
        Thread.setDefaultUncaughtExceptionHandler(previous)
        CrashLog.install(app)
        val handler = Thread.getDefaultUncaughtExceptionHandler()
        assertTrue(handler is CrashLog.Handler)
        assertSame(previous, (handler as CrashLog.Handler).previous)
        CrashLog.install(app) // idempotent: no handler chained to itself
        assertSame(handler, Thread.getDefaultUncaughtExceptionHandler())

        val error = boom("from a worker")
        val worker = Thread({}, "zeron-worker")
        handler.uncaughtException(worker, error)
        assertEquals(listOf(error), seen)
        val entry = CrashLog.pending(app)
        assertNotNull(entry)
        assertTrue(entry!!.text.contains("Thread: zeron-worker"))
        assertEquals("java.lang.IllegalStateException: from a worker", entry.headline)
    }

    @Test fun aFailingWriteStillReachesThePreviousHandler() {
        val seen = mutableListOf<Throwable>()
        // filesDir/crashes is a file, not a directory: writing fails.
        CrashLog.dir(app).apply { deleteRecursively(); parentFile?.mkdirs(); writeText("x") }
        val handler = CrashLog.Handler(app) { _, e -> seen += e }
        val error = RuntimeException("x")
        handler.uncaughtException(Thread.currentThread(), error)
        assertEquals(listOf(error), seen)
        CrashLog.dir(app).delete()
    }
}
