package sh.zeron.android

import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import java.io.File

/** The Compose compiler silently ignores a stability config entry that names no class. */
class ComposeStabilityConfigTest {
    @Test
    fun everyListedClassExists() {
        // Unit tests run from the module directory.
        val names = File("compose-stability.conf").readLines()
            .map { it.substringBefore("//").trim() }
            .filter { it.isNotEmpty() }
        assertTrue("no classes listed", names.isNotEmpty())
        for (name in names) {
            if ('*' in name) continue
            if (runCatching { Class.forName(name, false, javaClass.classLoader) }.isFailure) {
                fail("compose-stability.conf lists $name, which doesn't exist")
            }
        }
    }
}
