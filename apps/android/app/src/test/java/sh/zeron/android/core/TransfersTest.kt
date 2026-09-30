package sh.zeron.android.core

import org.json.JSONArray
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File
import java.nio.file.Files

class TransfersTest {
    @get:Rule val tmp = TemporaryFolder()

    @Test fun parsesEngineRows() {
        val rows = Transfers.list(
            JSONArray(
                """[{"id":"t1","direction":"incoming","peerDeviceId":"d1","peerDeviceName":"Host PC","state":"transferring",
                     "transport":"p2p","items":[{"name":"app.apk","kind":"file","size":2000000,"fileCount":1,"path":"/home/zeron/Zeron Transfers/Host PC/app.apk"},
                     {"name":"site","kind":"folder","size":3000,"fileCount":3}],
                     "fileCount":4,"totalBytes":2003000,"doneBytes":1001500,"bytesPerSec":5200000,"createdAt":1,"updatedAt":2},
                    {"id":"t2","direction":"outgoing","peerDeviceId":"d1","peerDeviceName":"Host PC","state":"awaitingAcceptance",
                     "items":[],"fileCount":0,"totalBytes":0,"doneBytes":0,"bytesPerSec":0,"createdAt":1,"updatedAt":2,"finishedAt":null},
                    {"id":"","direction":"incoming"},
                    {"id":"t3","direction":"incoming","state":"somethingNew","items":[],"createdAt":1,"updatedAt":2,"finishedAt":9,"error":"boom"}]""",
            ),
        )
        assertEquals(listOf("t1", "t2", "t3"), rows.map { it.id })
        val t1 = rows[0]
        assertTrue(t1.incoming)
        assertEquals(Transfers.State.Transferring, t1.state)
        assertEquals(0.5f, t1.fraction, 0.001f)
        assertEquals("app.apk and 1 more", t1.title)
        assertEquals(Transfers.Kind.Folder, t1.items[1].kind)
        assertNull(t1.items[1].path)
        assertEquals("1.0 MB of 2.0 MB · 5.2 MB/s · Direct", Transfers.detail(t1))
        assertEquals("Waiting for Host PC to accept", Transfers.stateLabel(rows[1]))
        assertNull(rows[1].finishedAt)
        assertTrue(rows[1].state.live)
        // Unknown states read as terminal failures, never as live forever.
        assertEquals(Transfers.State.Failed, rows[2].state)
        assertEquals("boom", Transfers.detail(rows[2]))
    }

    @Test fun wording() {
        assertEquals("999 B", Transfers.bytes(999))
        assertEquals("1.5 KB", Transfers.bytes(1500))
        assertEquals("250 MB", Transfers.bytes(250_000_000))
        assertEquals("1.1 GB", Transfers.bytes(1_100_000_000))
        assertEquals("1 file", Transfers.files(1))
        assertEquals("2 files", Transfers.files(2))
        assertEquals("Relayed", Transfers.transportLabel("relay"))
        assertNull(Transfers.transportLabel(null))
    }

    @Test fun guestPathsMapOntoTheRootfs() {
        val root = File("/data/user/0/sh.zeron.android/files/runtime/rootfs")
        val tmpDir = File("/data/user/0/sh.zeron.android/files/runtime/tmp")
        val paths = Transfers.GuestPaths(root, tmpDir)
        assertEquals(File(root, "home/zeron/Zeron Transfers/Host PC/a b.txt"), paths.host("/home/zeron/Zeron Transfers/Host PC/a b.txt"))
        assertEquals(File(tmpDir, "x/y"), paths.host("/tmp/x/y"))
        assertEquals(File(root, "tmpfoo"), paths.host("/tmpfoo"))
        assertEquals(root, paths.host("/"))
        assertNull(paths.host("relative/path"))
        assertNull(paths.host("/home/zeron/../../../etc/passwd"))
        assertEquals("/home/zeron/.zeron/outbox/b/f.txt", paths.guest(File(root, "home/zeron/.zeron/outbox/b/f.txt")))
        assertEquals("/tmp/q", paths.guest(File(tmpDir, "q")))
        assertNull(paths.guest(File("/data/user/0/sh.zeron.android/files/runtime/rootfs2/x")))
    }

    @Test fun outboxBatches() {
        assertEquals("/home/zeron/.zeron/outbox/abc", Transfers.outboxBatch("/home/zeron/.zeron/outbox/abc/file.txt"))
        assertEquals("/home/zeron/.zeron/outbox/abc", Transfers.outboxBatch("/home/zeron/.zeron/outbox/abc"))
        assertNull(Transfers.outboxBatch("/home/zeron/projects/app/file.txt"))
        assertNull(Transfers.outboxBatch("/home/zeron/.zeron/outbox/../x"))
        assertNull(Transfers.outboxBatch("/home/zeron/.zeron/outbox/"))
    }

    @Test fun downloadsRelativePaths() {
        assertEquals("Download/Zeron/", Transfers.relativePath(emptyList()))
        assertEquals("Download/Zeron/site/assets/", Transfers.relativePath(listOf("site", "assets")))
        assertEquals("Download/Zeron/a_b_c/", Transfers.relativePath(listOf("a:b?c")))
        assertEquals("_", Transfers.safeSegment(".."))
        assertEquals("_", Transfers.safeSegment("  "))
        assertEquals("notes", Transfers.safeSegment("notes..."))
    }

    @Test fun exportsKeepFolderStructureAndSkipSymlinks() {
        val folder = tmp.newFolder("site")
        File(folder, "index.html").writeText("<p>")
        File(folder, "assets").mkdirs()
        File(folder, "assets/app.js").writeText("1")
        File(folder, "assets/.app.js.zeron-t1.part").writeText("partial")
        Files.createSymbolicLink(File(folder, "link").toPath(), File("/etc/passwd").toPath())
        val isLink = { f: File -> Files.isSymbolicLink(f.toPath()) }
        val out = Transfers.exports(folder, isLink)
        assertEquals(
            listOf(listOf("site", "assets") to "app.js", listOf("site") to "index.html"),
            out.map { it.dirs to it.name },
        )
        val file = tmp.newFile("app.apk")
        assertEquals(listOf(emptyList<String>() to "app.apk"), Transfers.exports(file, isLink).map { it.dirs to it.name })
        assertTrue(Transfers.exports(File(folder, "link"), isLink).isEmpty())
        assertTrue(Transfers.exports(File(folder, "missing"), isLink).isEmpty())
    }

    @Test fun mimeTypes() {
        val platform = mapOf("pdf" to "application/pdf", "png" to "image/png", "txt" to "text/plain")
        val lookup = { ext: String -> platform[ext] }
        assertEquals(Transfers.APK_MIME, Transfers.mime("App-debug.APK", lookup))
        assertEquals("application/pdf", Transfers.mime("report.final.pdf", lookup))
        assertEquals("image/png", Transfers.mime("shot.PNG", lookup))
        assertEquals(Transfers.UNKNOWN_MIME, Transfers.mime("archive.xyz", lookup))
        assertEquals(Transfers.UNKNOWN_MIME, Transfers.mime("Makefile", lookup))
        assertEquals(Transfers.UNKNOWN_MIME, Transfers.mime(".bashrc", lookup))
        assertEquals(Transfers.UNKNOWN_MIME, Transfers.mime("trailing.", lookup))
        assertNull(Transfers.extension(".env"))
        assertEquals("gz", Transfers.extension("a.tar.gz"))
    }

    @Test fun terminalStates() {
        for (s in Transfers.State.entries) assertEquals(s.terminal, !s.live)
        assertFalse(Transfers.State.Reconnecting.terminal)
        assertTrue(Transfers.State.Declined.terminal)
    }
}
