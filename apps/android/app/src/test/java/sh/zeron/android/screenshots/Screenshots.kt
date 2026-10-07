package sh.zeron.android.screenshots

import org.junit.Assume.assumeTrue
import java.io.File

/**
 * Shared switches for the JVM screenshot renders. They only run with
 * `-PzeronScreenshots=true` (so the normal unit tests stay fast), and the
 * app renders need the host build of the Rust core in `target/mobile`.
 */
internal object Screenshots {
    val outDir: File
        get() = File(System.getProperty("zeron.screenshots.dir") ?: "build/screenshots").apply { mkdirs() }

    fun path(name: String): String = File(outDir, name).apply { parentFile?.mkdirs() }.absolutePath

    fun assumeEnabled() {
        assumeTrue("screenshots off (pass -PzeronScreenshots=true)", System.getProperty("zeron.screenshots") == "true")
    }

    fun assumeHostCore() {
        val dir = System.getProperty("jna.library.path").orEmpty()
        assumeTrue("host libzeron_mobile.so missing in $dir (see README)", File(dir, "libzeron_mobile.so").exists())
    }
}
