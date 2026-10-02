package sh.zeron.android.design

import androidx.compose.material3.ColorScheme
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.toArgb
import uniffi.zeron_core.ColorRole

/**
 * Transcript paint roles → ARGB for one appearance. Paint-only: nothing here
 * affects layout. Neutrals are Zeron's (the iOS palette); accent roles follow
 * the Material scheme so wallpaper colors flow into links and quotes.
 */
class TranscriptPalette(dark: Boolean, scheme: ColorScheme) {
    private val colors = IntArray(ColorRole.entries.size)

    val background: Int = scheme.background.toArgb()

    operator fun get(role: ColorRole): Int = colors[role.ordinal]

    init {
        fun c(light: Long, darkHex: Long, alpha: Float = 1f): Int {
            val base = Color(if (dark) darkHex or 0xFF000000 else light or 0xFF000000)
            return base.copy(alpha = alpha).toArgb()
        }
        fun w(lightAlpha: Float, darkAlpha: Float): Int =
            if (dark) Color.White.copy(alpha = darkAlpha).toArgb() else Color.Black.copy(alpha = lightAlpha).toArgb()
        val accent = scheme.primary
        for (role in ColorRole.entries) {
            colors[role.ordinal] = when (role) {
                ColorRole.TEXT -> scheme.onSurface.toArgb()
                ColorRole.TEXT_SECONDARY -> c(0x62626A, 0xA9A9AE)
                ColorRole.TEXT_TERTIARY -> c(0x97979F, 0x6B6B72)
                ColorRole.LINK, ColorRole.ACCENT -> accent.toArgb()
                ColorRole.DANGER -> c(0xDC2626, 0xF87171)
                ColorRole.SUCCESS -> c(0x15803D, 0x34D399)
                ColorRole.WARNING -> c(0xA16207, 0xFACC15)
                ColorRole.INLINE_CODE_TEXT -> c(0x3F3F46, 0xDCDCE0)
                ColorRole.INLINE_CODE_BACKGROUND -> c(0xE9E9ED, 0x1A1A1E)
                ColorRole.CODE_TEXT -> c(0x303035, 0xE8E8EA)
                ColorRole.CODE_BACKGROUND -> c(0xFAFAFB, 0x0B0B0D)
                ColorRole.CODE_BORDER -> c(0xE4E4E8, 0x1F1F23)
                ColorRole.QUOTE_BAR -> accent.copy(alpha = 0.45f).toArgb()
                ColorRole.RULE -> c(0xE2E2E6, 0x1E1E22)
                ColorRole.TABLE_BORDER -> c(0xE2E2E6, 0x232327)
                ColorRole.TABLE_HEADER_BACKGROUND -> c(0xF3F3F5, 0x121215)
                ColorRole.USER_BUBBLE -> scheme.surfaceContainerHigh.toArgb()
                ColorRole.CHIP_BACKGROUND -> c(0xE7E7EB, 0x1C1C20)
                ColorRole.SYNTAX_KEYWORD -> c(0x5B43E8, 0x8B7CF6)
                ColorRole.SYNTAX_STRING -> c(0x15803D, 0x34D399)
                ColorRole.SYNTAX_COMMENT -> c(0x6B7280, 0x92929A)
                ColorRole.SYNTAX_NUMBER, ColorRole.SYNTAX_CONSTANT -> c(0xA16207, 0xFACC15)
                ColorRole.SYNTAX_FUNCTION -> c(0x2563EB, 0x60A5FA)
                ColorRole.SYNTAX_TYPE -> c(0x7E22CE, 0xC084FC)
                ColorRole.SYNTAX_VARIABLE -> c(0x303035, 0xE8E8EA)
                ColorRole.SYNTAX_PROPERTY -> c(0x0E7490, 0x22D3EE)
                ColorRole.SYNTAX_OPERATOR, ColorRole.SYNTAX_PUNCTUATION -> c(0x52525B, 0xA1A1AA)
                ColorRole.SYNTAX_TAG -> c(0xBE185D, 0xF472B6)
                ColorRole.SYNTAX_ATTRIBUTE -> c(0xB91C1C, 0xF87171)
                ColorRole.SYNTAX_ESCAPE -> c(0x0E7490, 0x22D3EE)
                ColorRole.TEXT_FAINT -> c(0x797981, 0x85858A)
                ColorRole.TEXT_SOFT -> c(0x303035, 0xE8E8EA, 0.85f)
                ColorRole.TOOL_RAIL -> w(0.162f, 0.12f)
                ColorRole.TOOL_BADGE -> w(0.06f, 0.06f)
                ColorRole.TOOL_WELL -> if (dark) Color.Black.copy(alpha = 0.16f).toArgb() else Color.White.copy(alpha = 0.16f).toArgb()
                ColorRole.AGENT_CARD -> w(0.03f, 0.03f)
                ColorRole.AGENT_CARD_BORDER -> w(0.0945f, 0.07f)
                ColorRole.AGENT_TILE -> w(0.08f, 0.08f)
                ColorRole.DIFF_ADD_WASH -> c(0x15803D, 0x34D399, 0.055f)
                ColorRole.DIFF_DEL_WASH -> c(0xDC2626, 0xF87171, 0.055f)
                ColorRole.DIFF_ADD_BAR -> c(0x15803D, 0x34D399, 0.55f)
                ColorRole.DIFF_DEL_BAR -> c(0xDC2626, 0xF87171, 0.55f)
                ColorRole.DIFF_HUNK -> accent.copy(alpha = if (dark) 0.08f else 0.07f).toArgb()
            }
        }
    }
}

/** Project tones — the desktop's monogram palette, by `project_color_index`. */
object ProjectColors {
    private val light = longArrayOf(0x475569, 0x2563EB, 0x7C3AED, 0xBE123C, 0xA16207, 0x047857, 0x0F766E, 0xC2410C)
    private val dark = longArrayOf(0x94A3B8, 0x93C5FD, 0xC4B5FD, 0xFDA4AF, 0xFCD34D, 0x6EE7B7, 0x5EEAD4, 0xFDBA74)

    fun color(index: Int, isDark: Boolean): Color {
        val table = if (isDark) dark else light
        return Color(table[Math.floorMod(index, table.size)] or 0xFF000000)
    }
}
