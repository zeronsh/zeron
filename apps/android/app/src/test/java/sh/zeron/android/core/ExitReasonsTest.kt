package sh.zeron.android.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File
import java.nio.file.Files

class ExitReasonsTest {
    private fun exit(timestamp: Long, reason: Int = 6, description: String? = "user request after error") =
        ExitReasons.Exit(timestamp, reason, 0, 100, 123_456, 234_567, description, "sh.zeron.android")

    @Test
    fun only_exits_after_the_newest_logged_one_are_new_oldest_first() {
        val exits = listOf(exit(300), exit(100), exit(200), exit(400))
        assertEquals(listOf(300L, 400L), ExitReasons.unseen(exits, 200).map { it.timestamp })
        assertEquals(listOf(100L, 200L, 300L, 400L), ExitReasons.unseen(exits, 0).map { it.timestamp })
        assertTrue(ExitReasons.unseen(exits, 400).isEmpty())
    }

    @Test
    fun an_exit_reads_as_one_line() {
        val line = ExitReasons.describe(exit(0, description = "Input dispatching timed out\n(waited 5s)"))
        assertEquals(
            "previous exit 1970-01-01T00:00:00.000Z reason=ANR status=0 importance=100 pss=123456kB rss=234567kB " +
                "process=sh.zeron.android description=\"Input dispatching timed out (waited 5s)\"",
            line,
        )
    }

    @Test
    fun reasons_are_named() {
        assertEquals("CRASH", ExitReasons.reasonName(4))
        assertEquals("CRASH_NATIVE", ExitReasons.reasonName(5))
        assertEquals("LOW_MEMORY", ExitReasons.reasonName(3))
        assertEquals("EXCESSIVE_RESOURCE_USAGE", ExitReasons.reasonName(9))
        assertEquals("REASON_99", ExitReasons.reasonName(99))
    }

    @Test
    fun an_anr_trace_gives_the_main_threads_section() {
        val trace = """
            ----- pid 1234 at 2026-11-20 10:00:00 -----
            Cmd line: sh.zeron.android

            "Signal Catcher" daemon prio=10 tid=2 Runnable
              at nothing

            "main" prio=5 tid=1 Blocked
              at uniffi.zeron_core.SessionHandle.send(zeron_core.kt:100)
              at sh.zeron.android.ui.ComposerKt.send(Composer.kt:213)

            "zeron-send-1" prio=5 tid=20 Native
              at somewhere
        """.trimIndent()
        val excerpt = ExitReasons.excerpt(trace.byteInputStream())
        assertEquals(
            """
            "main" prio=5 tid=1 Blocked
              at uniffi.zeron_core.SessionHandle.send(zeron_core.kt:100)
              at sh.zeron.android.ui.ComposerKt.send(Composer.kt:213)
            """.trimIndent(),
            excerpt,
        )
    }

    @Test
    fun a_trace_excerpt_is_bounded() {
        val main = "\"main\" prio=5 tid=1 Blocked\n" + (1..1000).joinToString("\n") { "  at frame$it" }
        val byLines = ExitReasons.excerpt(main.byteInputStream(), maxLines = 50)
        assertEquals(51, byLines.lines().size)
        assertTrue(byLines.endsWith("…"))
        val byBytes = ExitReasons.excerpt(main.byteInputStream(), maxBytes = 1024)
        assertTrue(byBytes.length <= 1024 + 2)
        // A tombstone has no main section: its top is what's kept.
        val tombstone = (1..1000).joinToString("\n") { "#$it pc 0000 libzeron_mobile.so" }
        val top = ExitReasons.excerpt(tombstone.byteInputStream(), maxLines = 10)
        assertEquals("#1 pc 0000 libzeron_mobile.so", top.lines().first())
        assertEquals(11, top.lines().size)
    }

    @Test
    fun the_log_file_keeps_only_its_newest_whole_lines() {
        val dir = Files.createTempDirectory(File("build").apply { mkdirs() }.toPath(), "exits").toFile()
        val file = File(dir, ExitReasons.FILE_NAME)
        for (i in 1..100) ExitReasons.append(file, "line $i\n", limit = 200)
        val text = file.readText()
        assertTrue(text.length <= 200)
        assertTrue(text.startsWith("line "))
        assertTrue(text.endsWith("line 100\n"))
        dir.deleteRecursively()
    }
}
