package sh.zeron.android.design

import androidx.compose.ui.graphics.Color
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

class ZeronThemesTest {
    private fun hex(c: Color) = "%06X".format(Rgb.of(c))

    @Test
    fun registryMatchesTheDesktop() {
        // crates/theme/src/builtins.rs at ed3b1aae: 30 variants (10 light, 20 dark), Zeron first.
        assertEquals(30, ZeronThemes.all.size)
        assertEquals(10, ZeronThemes.variants(dark = false).size)
        assertEquals(20, ZeronThemes.variants(dark = true).size)
        assertEquals("zeron-light", ZeronThemes.variants(false).first().id)
        assertEquals("zeron-dark", ZeronThemes.variants(true).first().id)
        assertEquals(ZeronThemes.all.size, ZeronThemes.all.map { it.id }.toSet().size)
    }

    @Test
    fun accentPresetsAreTheDesktopsColors() {
        assertEquals(listOf("Theme default", "Zeron", "Orange", "Amber", "Green", "Cyan", "Blue", "Pink"), AccentChoice.entries.map { it.label })
        assertEquals(0x8B7CF6, AccentChoice.ZERON.hex(dark = true))
        assertEquals(0x5B43E8, AccentChoice.ZERON.hex(dark = false))
        assertEquals(0xFB923C, AccentChoice.ORANGE.hex(true))
        assertEquals(0x2563EB, AccentChoice.BLUE.hex(false))
        assertEquals(null, AccentChoice.THEME.hex(true))
        assertEquals(AccentChoice.THEME, AccentChoice.of(null))
        assertEquals(AccentChoice.PINK, AccentChoice.of("pink"))
    }

    @Test
    fun defaultsAreTheHandTunedZeronPalettes() {
        assertSame(ZeronLight, ZeronThemes.colors(false, null, null, AccentChoice.THEME))
        assertSame(ZeronDark, ZeronThemes.colors(true, "missing", "missing", AccentChoice.THEME))
        assertEquals("5B43E8", hex(ZeronLight.accent))
        assertEquals("8B7CF6", hex(ZeronDark.accent))
    }

    @Test
    fun accentRolesFollowTheRust() {
        val light = AccentRoles.derive(0x5B43E8, dark = false, background = 0xFFFFFF)
        assertEquals(listOf("7965EC", "5B43E8", "4332AC"), light.glyph.map { "%06X".format(it) })
        // A low-contrast preset is pushed until it clears 3:1 against the page.
        val amber = AccentRoles.derive(0xA16207, dark = false, background = 0xFFFFFF)
        assertTrue(Rgb.contrast(amber.primary, 0xFFFFFF) >= 3.0f)
    }

    @Test
    fun accentOverrideLeavesSemanticColorsAlone() {
        for (dark in listOf(false, true)) {
            val base = ZeronThemes.colors(dark, null, null, AccentChoice.THEME)
            val orange = ZeronThemes.colors(dark, null, null, AccentChoice.ORANGE)
            assertNotEquals(base.accent, orange.accent)
            assertEquals(base.success, orange.success)
            assertEquals(base.warning, orange.warning)
            assertEquals(base.danger, orange.danger)
            assertEquals(base.background, orange.background)
        }
    }

    @Test
    fun derivedThemesUseTheirSeeds() {
        val mocha = ZeronThemes.colors(true, null, "catppuccin-mocha", AccentChoice.THEME)
        val seed = ZeronThemes.seed("catppuccin-mocha", true)
        assertEquals("catppuccin-mocha", mocha.themeId)
        assertEquals("%06X".format(seed.background), hex(mocha.background))
        assertTrue(mocha.dark)
        assertTrue(mocha.syntax != null)
        // A dark id asked for in light falls back to the light default.
        assertSame(ZeronLight, ZeronThemes.colors(false, "catppuccin-mocha", null, AccentChoice.THEME))
    }
}
