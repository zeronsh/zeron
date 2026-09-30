package sh.zeron.android.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class ProjectNamesTest {
    @Test fun cloneTargets() {
        assertEquals("widgets", ProjectNames.repoName("https://github.com/acme/widgets.git"))
        assertEquals("widgets", ProjectNames.repoName("git@github.com:acme/widgets.git"))
        assertEquals("dot.files", ProjectNames.repoName("https://example.com/me/dot.files/"))
        assertNull(ProjectNames.repoName("widgets"))
        assertNull(ProjectNames.repoName("https://example.com/a b"))
        assertNull(ProjectNames.repoName("https://example.com/.."))
    }

    @Test fun folderNames() {
        assertEquals("my-app", ProjectNames.folderName(" my-app "))
        assertNull(ProjectNames.folderName("a/b"))
        assertNull(ProjectNames.folderName(""))
    }
}
