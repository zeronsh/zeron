package sh.zeron.android.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** Computer-file references: workspace-relative mention links inside the session folder, absolute paths outside. */
class PcFileRefsTest {
    private val link: (String, Boolean) -> String = { p, _ -> "[${p.substringAfterLast('/')}](zeron-file:$p)" }

    @Test
    fun relativeToPosix() {
        assertEquals("README.md", HostPaths.relativeTo("/Users/dev/zeron", "/Users/dev/zeron/README.md"))
        assertEquals("docs/release.md", HostPaths.relativeTo("/Users/dev/zeron/", "/Users/dev/zeron/docs/release.md"))
        // Not inside: a sibling sharing the prefix, the folder itself, elsewhere.
        assertNull(HostPaths.relativeTo("/Users/dev/zeron", "/Users/dev/zeron-ios/a.txt"))
        assertNull(HostPaths.relativeTo("/Users/dev/zeron", "/Users/dev/zeron"))
        assertNull(HostPaths.relativeTo("/Users/dev/zeron", "/Users/dev/todo.md"))
        assertEquals("etc/hosts", HostPaths.relativeTo("/", "/etc/hosts"))
    }

    @Test
    fun relativeToWindows() {
        assertEquals("src/main.rs", HostPaths.relativeTo("C:\\Users\\me\\proj", "C:\\Users\\me\\proj\\src\\main.rs"))
        // Drive letters and folders compare case-insensitively.
        assertEquals("a.txt", HostPaths.relativeTo("c:\\users\\ME\\proj", "C:\\Users\\me\\proj\\a.txt"))
        assertEquals("a.txt", HostPaths.relativeTo("C:\\", "C:\\a.txt"))
        assertNull(HostPaths.relativeTo("C:\\Users\\me\\proj", "D:\\proj\\a.txt"))
    }

    @Test
    fun referencesInsideAndOutsideTheSessionFolder() {
        assertEquals("[README.md](zeron-file:README.md)", PcFileRefs.reference("/Users/dev/zeron", "/Users/dev/zeron/README.md", link))
        assertEquals("[a.txt](zeron-file:src/a.txt)", PcFileRefs.reference("C:\\p", "C:\\p\\src\\a.txt", link))
        assertEquals("`/Users/dev/todo.md`", PcFileRefs.reference("/Users/dev/zeron", "/Users/dev/todo.md", link))
        assertEquals("`D:\\x.txt`", PcFileRefs.reference("C:\\p", "D:\\x.txt", link))
        // No usable session folder: absolute.
        assertEquals("`/Users/dev/todo.md`", PcFileRefs.reference("~", "/Users/dev/todo.md", link))
        assertEquals("`/Users/dev/todo.md`", PcFileRefs.reference(null, "/Users/dev/todo.md", link))
        assertEquals("`` /tmp/a`b ``", PcFileRefs.reference(null, "/tmp/a`b", link))
    }

    @Test
    fun insertAppendsSpaceSeparated() {
        assertEquals("a b ", PcFileRefs.insert("", listOf("a", "b")))
        assertEquals("look at a ", PcFileRefs.insert("look at", listOf("a")))
        assertEquals("look at a ", PcFileRefs.insert("look at ", listOf("a")))
        assertEquals("x", PcFileRefs.insert("x", emptyList()))
    }
}
