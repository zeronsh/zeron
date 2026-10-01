package sh.zeron.android.tools

import android.view.KeyEvent
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.zeron_core.TerminalKey

class TerminalLogicTest {
    @Test fun gridFitsWholeCells() {
        // 8 px padding each side: 400 - 16 = 384 / 10 = 38.4 → 38 cols; 300 - 16 = 284 / 20 → 14 rows.
        assertEquals(38 to 14, terminalGrid(400, 300, 8f, 10f, 20f))
        assertEquals("never below 2×1", 2 to 1, terminalGrid(10, 10, 8f, 10f, 20f))
    }

    @Test fun hardwareKeys() {
        assertEquals(TerminalKey.BACKSPACE, terminalKey(KeyEvent.KEYCODE_DEL))
        assertEquals(TerminalKey.DELETE, terminalKey(KeyEvent.KEYCODE_FORWARD_DEL))
        assertEquals(TerminalKey.ENTER, terminalKey(KeyEvent.KEYCODE_NUMPAD_ENTER))
        assertEquals(TerminalKey.LEFT, terminalKey(KeyEvent.KEYCODE_DPAD_LEFT))
        assertEquals(TerminalKey.HOME, terminalKey(KeyEvent.KEYCODE_MOVE_HOME))
        assertEquals(TerminalKey.PAGE_DOWN, terminalKey(KeyEvent.KEYCODE_PAGE_DOWN))
        assertEquals(TerminalKey.F1, terminalKey(KeyEvent.KEYCODE_F1))
        assertEquals(TerminalKey.F12, terminalKey(KeyEvent.KEYCODE_F12))
        assertNull("text keys go through unicodeChar", terminalKey(KeyEvent.KEYCODE_A))
        assertNull("Back stays the system's", terminalKey(KeyEvent.KEYCODE_BACK))
    }

    @Test fun stickyModifiersDisarmAfterOneUse() {
        val sticky = StickyKeys()
        sticky.ctrl = true
        assertEquals(true to false, sticky.take())
        assertEquals(false to false, sticky.take())
        assertFalse(sticky.ctrl)
    }

    @Test fun paletteFollowsTheme() {
        val dark = terminalPalette(dark = true, background = 0xFF060606.toInt(), cursor = 0xFF8B7CF6.toInt())
        assertEquals(16, dark.ansi.size)
        assertEquals(0xFF060606u, dark.background)
        assertFalse(dark.light)
        val light = terminalPalette(dark = false, background = 0xFFF3F3F5.toInt(), cursor = 0xFF5B43E8.toInt())
        assertTrue(light.light)
        assertEquals(0xFFDC2626u, light.ansi[1])
    }

    @Test fun tabLabelPrefersTitle() {
        assertEquals("zsh", TerminalTab("t1", "zsh", "/home").label)
        assertEquals("vim", TerminalTab("t1", "zsh", "/home", title = "vim").label)
        assertEquals("zsh", TerminalTab("t1", "zsh", "/home", title = " ").label)
    }
}
