package sh.zeron.android.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.zeron_core.CoreException
import uniffi.zeron_core.WorkspaceFile

class FilePreviewsTest {
    @Test
    fun titleIsTheFileNameWithoutLineRefs() {
        assertEquals("report.md", FilePreviews.title("/home/me/proj/docs/report.md:12"))
        assertEquals("main.rs", FilePreviews.title("src/main.rs:3:7"))
        assertEquals("设计.md", FilePreviews.title("D:\\Words\\Duck\\docs\\设计.md"))
        assertEquals("设计 稿.md", FilePreviews.title("file:///d:/x/%E8%AE%BE%E8%AE%A1%20%E7%A8%BF.md#L3"))
        assertEquals("a+b.md", FilePreviews.title("a+b.md"))
    }

    @Test
    fun markdownByExtension() {
        assertTrue(FilePreviews.isMarkdown("docs/README.MD"))
        assertTrue(FilePreviews.isMarkdown("notes.markdown:4"))
        assertFalse(FilePreviews.isMarkdown("src/main.rs"))
        assertFalse(FilePreviews.isMarkdown("Makefile"))
    }

    @Test
    fun statesFromTheRead() {
        val md = WorkspaceFile("docs/a.md", "# A", 3uL, false)
        assertEquals(FilePreviewState.Text(md, markdown = true), FilePreviews.loaded(md, "zeron-file:docs/a.md"))
        // The link's own name is the fallback when the resolved path loses
        // the extension (a renamed temp path, a bare `pending:` ref…).
        val resolved = WorkspaceFile("blob-9f2c", "# A", 3uL, false)
        assertEquals(FilePreviewState.Text(resolved, markdown = true), FilePreviews.loaded(resolved, "zeron-file:docs/a.md"))
        assertEquals(FilePreviewState.Text(resolved, markdown = false), FilePreviews.loaded(resolved, "zeron-file:docs/a.txt"))
        val bin = WorkspaceFile("logo.png", null, 2048uL, false)
        assertEquals(FilePreviewState.Binary(bin), FilePreviews.loaded(bin, "zeron-file:logo.png"))
        assertEquals(
            FilePreviewState.Failed(outside = true, message = null),
            FilePreviews.failed(CoreException.InvalidArgument("outside")),
        )
        assertEquals(
            FilePreviewState.Failed(outside = false, message = "offline"),
            FilePreviews.failed(CoreException.HostUnavailable("offline")),
        )
    }
}
