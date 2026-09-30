package sh.zeron.android.tools

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class ToolsTest {
    private val ref = WorkspaceRef("dev-1", "chat-1", "space-1", "/home/zeron/projects/site", "site", "Pixel")

    @Test
    fun linksOpenPagesInTheBrowserAndFilesInTheViewer() {
        assertEquals(Links.Target.Web("http://localhost:8000/"), Links.classify("http://localhost:8000/", ref))
        assertEquals(Links.Target.Web("https://example.com"), Links.classify("https://example.com", ref))
        assertEquals(Links.Target.File("out/report.pdf"), Links.classify("/home/zeron/projects/site/out/report.pdf", ref))
        assertEquals(Links.Target.File("index.html"), Links.classify("./index.html", ref))
        assertEquals(Links.Target.File("src/main.rs"), Links.classify("src/main.rs:12:4", ref))
        assertEquals(Links.Target.File("docs/a b.md"), Links.classify("file:///home/zeron/projects/site/docs/a%20b.md", ref))
        assertEquals(Links.Target.File("README.md"), Links.classify("README.md#L3", ref))
        assertEquals(Links.Target.Outside("/etc/hosts"), Links.classify("/etc/hosts", ref))
        assertEquals(Links.Target.Outside("../other/x"), Links.classify("../other/x", ref))
        assertEquals(Links.Target.Outside("/home/zeron/projects/site-2/x"), Links.classify("/home/zeron/projects/site-2/x", ref))
        assertEquals(Links.Target.Other, Links.classify("mailto:a@b.c", ref))
        assertEquals(Links.Target.Other, Links.classify("report.pdf", null))
    }

    @Test
    fun fileKindsPickTheViewer() {
        assertEquals(FileKind.Pdf, FileKind.of("out/Report.PDF"))
        assertEquals(FileKind.Image, FileKind.of("a/b.png"))
        assertEquals(FileKind.Svg, FileKind.of("logo.svg"))
        assertEquals(FileKind.Markdown, FileKind.of("README.md"))
        assertEquals(FileKind.Html, FileKind.of("index.html"))
        assertEquals(FileKind.Binary, FileKind.of("app-debug.apk"))
        assertEquals(FileKind.Text, FileKind.of("Makefile"))
        assertEquals(FileKind.Text, FileKind.of("src/lib.rs"))
    }

    @Test
    fun gitMarksFollowTheDesktopOrderAndBubbleUp() {
        val frame = JSONObject(
            """{"status":{"files":[
              {"path":"src/a.rs","index":"unchanged","worktree":"modified"},
              {"path":"src/new.rs","index":"unchanged","worktree":"untracked"},
              {"path":"src/deep/gone.rs","index":"deleted","worktree":"unchanged"},
              {"path":"ok.rs","index":"unchanged","worktree":"unchanged"}]}}""",
        )
        val marks = WorkspaceApi.gitMarks(frame)!!
        assertEquals('M', marks["src/a.rs"]!!.letter)
        assertEquals('U', marks["src/new.rs"]!!.letter)
        assertEquals('D', marks["src/deep/gone.rs"]!!.letter)
        assertNull(marks["ok.rs"])
        val folders = WorkspaceApi.folderMarks(marks)
        assertEquals(GitMark.Kind.Deleted, folders["src"]!!.kind)
        assertEquals(GitMark.Kind.Deleted, folders["src/deep"]!!.kind)
        assertNull(WorkspaceApi.gitMarks(JSONObject("""{"status":null}""")))
        assertEquals('!', WorkspaceApi.gitMark("unmerged", "modified")!!.letter)
    }

    @Test
    fun treeRowsWalkOnlyExpandedFolders() {
        fun dir(p: String) = Entry(p, p.substringAfterLast('/'), true, null, false, false)
        fun file(p: String) = Entry(p, p.substringAfterLast('/'), false, 1, false, false)
        val children = mapOf(
            "" to listOf(dir("src"), dir("docs"), file("README.md")),
            "src" to listOf(dir("src/bin"), file("src/lib.rs")),
            "src/bin" to listOf(file("src/bin/main.rs")),
            "docs" to listOf(file("docs/a.md")),
        )
        val rows = visibleRows(children, listOf("src", "src/bin"))
        assertEquals(listOf("src", "src/bin", "src/bin/main.rs", "src/lib.rs", "docs", "README.md"), rows.map { it.entry.path })
        assertEquals(listOf(0, 1, 2, 1, 0, 0), rows.map { it.depth })
    }

    @Test
    fun addressBarLikeTheDesktop() {
        assertEquals("http://localhost:8000", Browser.normalize("8000"))
        assertEquals("http://localhost:5173/app", Browser.normalize("localhost:5173/app"))
        assertEquals("http://phone.site.localhost:7331", Browser.normalize("phone.site.localhost:7331"))
        assertEquals("http://127.0.0.1:3000", Browser.normalize("127.0.0.1:3000"))
        assertEquals("https://example.com/x", Browser.normalize("example.com/x"))
        assertEquals("https://example.com", Browser.normalize("https://example.com"))
    }

    @Test
    fun previewsParseTheEngineSnapshot() {
        val p = Browser.previews(
            JSONObject(
                """{"services":[{"id":"s1","name":"Vite","hostname":"phone.site.localhost","port":5173,"deviceName":"Pixel","projectName":"site"}],
                   "proxyPort":7331,"error":null}""",
            ),
        )
        assertEquals(1, p.services.size)
        assertEquals("http://phone.site.localhost:7331", p.services[0].url(p.proxyPort))
        assertNull(p.error)
    }
}
