package sh.zeron.android.design

import androidx.compose.runtime.Immutable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color
import uniffi.zeron_core.ColorRole

/**
 * Zeron Light / Zeron Dark, copied from the iOS palette (cool neutrals, violet accent).
 * Paint only — layout numbers live with the screens.
 */
@Immutable
data class ZeronColors(
    val dark: Boolean,
    val background: Color,
    val elevated: Color,
    val text: Color,
    val secondary: Color,
    val tertiary: Color,
    val hairline: Color,
    val accent: Color,
    val controlFill: Color,
    val accentSoft: Color,
    val danger: Color,
    val success: Color,
    val warning: Color,
    val userBubble: Color,
    val codeBackground: Color,
    val codeBorder: Color,
    val chip: Color,
    val subline: Color,
    val rowActive: Color,
    val projectTones: List<Color>,
    val working: Color,
    val input: Color,
    val failed: Color,
    val done: Color,
    val time: Color,
) {
    fun of(role: ColorRole): Color = when (role) {
        ColorRole.TEXT -> text
        ColorRole.TEXT_SECONDARY -> secondary
        ColorRole.TEXT_TERTIARY -> tertiary
        ColorRole.LINK, ColorRole.ACCENT -> accent
        ColorRole.DANGER -> danger
        ColorRole.SUCCESS -> success
        ColorRole.WARNING -> warning
        // iOS Palette: gray inline code (no accent tint).
        ColorRole.INLINE_CODE_TEXT -> if (dark) Color(0xFFDCDCE0) else Color(0xFF3F3F46)
        ColorRole.INLINE_CODE_BACKGROUND -> if (dark) Color(0xFF1A1A1E) else Color(0xFFE9E9ED)
        ColorRole.CODE_TEXT -> if (dark) Color(0xFFE8E8EA) else Color(0xFF303035)
        ColorRole.CODE_BACKGROUND -> codeBackground
        ColorRole.CODE_BORDER -> codeBorder
        ColorRole.QUOTE_BAR -> accent.copy(alpha = 0.45f)
        ColorRole.RULE -> hairline
        ColorRole.TABLE_BORDER -> if (dark) Color(0xFF232327) else Color(0xFFE2E2E6)
        ColorRole.TABLE_HEADER_BACKGROUND -> if (dark) Color(0xFF121215) else Color(0xFFF3F3F5)
        ColorRole.USER_BUBBLE -> userBubble
        ColorRole.CHIP_BACKGROUND -> chip
        ColorRole.SYNTAX_KEYWORD -> accent
        ColorRole.SYNTAX_STRING -> success
        ColorRole.SYNTAX_COMMENT -> if (dark) Color(0xFF92929A) else Color(0xFF6B7280)
        ColorRole.SYNTAX_NUMBER, ColorRole.SYNTAX_CONSTANT -> warning
        ColorRole.SYNTAX_FUNCTION -> if (dark) Color(0xFF60A5FA) else Color(0xFF2563EB)
        ColorRole.SYNTAX_TYPE -> if (dark) Color(0xFFC084FC) else Color(0xFF7E22CE)
        ColorRole.SYNTAX_VARIABLE -> if (dark) Color(0xFFE8E8EA) else Color(0xFF303035)
        ColorRole.SYNTAX_PROPERTY, ColorRole.SYNTAX_ESCAPE -> if (dark) Color(0xFF22D3EE) else Color(0xFF0E7490)
        ColorRole.SYNTAX_OPERATOR, ColorRole.SYNTAX_PUNCTUATION -> if (dark) Color(0xFFA1A1AA) else Color(0xFF52525B)
        ColorRole.SYNTAX_TAG -> if (dark) Color(0xFFF472B6) else Color(0xFFBE185D)
        ColorRole.SYNTAX_ATTRIBUTE -> if (dark) Color(0xFFF87171) else Color(0xFFB91C1C)
        ColorRole.TEXT_FAINT -> if (dark) Color(0xFF85858A) else Color(0xFF797981)
        ColorRole.TEXT_SOFT -> text.copy(alpha = 0.85f)
        ColorRole.TOOL_RAIL -> if (dark) Color.White.copy(alpha = 0.12f) else Color.Black.copy(alpha = 0.162f)
        ColorRole.TOOL_BADGE -> if (dark) Color.White.copy(alpha = 0.06f) else Color.Black.copy(alpha = 0.06f)
        ColorRole.TOOL_WELL -> if (dark) Color.Black.copy(alpha = 0.16f) else Color.White.copy(alpha = 0.16f)
        ColorRole.AGENT_CARD -> if (dark) Color.White.copy(alpha = 0.03f) else Color.Black.copy(alpha = 0.03f)
        ColorRole.AGENT_CARD_BORDER -> if (dark) Color.White.copy(alpha = 0.07f) else Color.Black.copy(alpha = 0.0945f)
        ColorRole.AGENT_TILE -> if (dark) Color.White.copy(alpha = 0.08f) else Color.Black.copy(alpha = 0.08f)
        ColorRole.DIFF_ADD_WASH -> success.copy(alpha = 0.055f)
        ColorRole.DIFF_DEL_WASH -> danger.copy(alpha = 0.055f)
        ColorRole.DIFF_ADD_BAR -> success.copy(alpha = 0.55f)
        ColorRole.DIFF_DEL_BAR -> danger.copy(alpha = 0.55f)
        ColorRole.DIFF_HUNK -> accent.copy(alpha = if (dark) 0.08f else 0.07f)
    }

    fun project(index: Int): Color = projectTones[index.mod(projectTones.size)]

    /** Claude keeps its brand orange; other marks take the foreground. */
    fun brandTint(harness: String?): Color = when (harness) {
        "claude-code", "mock", null -> Color(0xFFD97757)
        else -> text
    }
}

private val lightTones = listOf(
    Color(0xFF475569), Color(0xFF2563EB), Color(0xFF7C3AED), Color(0xFFBE123C),
    Color(0xFFA16207), Color(0xFF047857), Color(0xFF0F766E), Color(0xFFC2410C),
)
private val darkTones = listOf(
    Color(0xFF94A3B8), Color(0xFF93C5FD), Color(0xFFC4B5FD), Color(0xFFFDA4AF),
    Color(0xFFFCD34D), Color(0xFF6EE7B7), Color(0xFF5EEAD4), Color(0xFFFDBA74),
)

val ZeronLight = ZeronColors(
    dark = false,
    background = Color(0xFFF3F3F5),
    elevated = Color.White,
    text = Color(0xFF27272C),
    secondary = Color(0xFF62626A),
    tertiary = Color(0xFF97979F),
    hairline = Color(0xFFE2E2E6),
    accent = Color(0xFF5B43E8),
    controlFill = Color(0xFF27272C).copy(alpha = 0.075f),
    accentSoft = Color(0xFF5B43E8).copy(alpha = 0.12f),
    danger = Color(0xFFDC2626),
    success = Color(0xFF15803D),
    warning = Color(0xFFA16207),
    userBubble = Color.White,
    codeBackground = Color(0xFFFAFAFB),
    codeBorder = Color(0xFFE4E4E8),
    chip = Color(0xFFE7E7EB),
    subline = Color(0xFF62626A).copy(alpha = 0.5f),
    rowActive = Color.White.copy(alpha = 0.9f),
    projectTones = lightTones,
    working = Color(0xFF5B43E8).copy(alpha = 0.55f),
    input = Color(0xFF5B43E8).copy(alpha = 0.6f),
    failed = Color(0xFFDC2626).copy(alpha = 0.65f),
    done = Color(0xFF15803D).copy(alpha = 0.9f),
    time = Color(0xFF62626A).copy(alpha = 0.5f),
)

val ZeronDark = ZeronColors(
    dark = true,
    background = Color(0xFF060606),
    elevated = Color(0xFF111113),
    text = Color(0xFFE8E8EA),
    secondary = Color(0xFFA9A9AE),
    tertiary = Color(0xFF6B6B72),
    hairline = Color(0xFF1E1E22),
    accent = Color(0xFF8B7CF6),
    controlFill = Color(0xFFE8E8EA).copy(alpha = 0.075f),
    accentSoft = Color(0xFF8B7CF6).copy(alpha = 0.12f),
    danger = Color(0xFFF87171),
    success = Color(0xFF34D399),
    warning = Color(0xFFFACC15),
    userBubble = Color(0xFF19191C),
    codeBackground = Color(0xFF0B0B0D),
    codeBorder = Color(0xFF1F1F23),
    chip = Color(0xFF1C1C20),
    subline = Color(0xFFA9A9AE).copy(alpha = 0.5f),
    rowActive = Color.White.copy(alpha = 0.06f),
    projectTones = darkTones,
    working = Color(0xFF8B7CF6).copy(alpha = 0.55f),
    input = Color(0xFF8B7CF6).copy(alpha = 0.6f),
    failed = Color(0xFFF87171).copy(alpha = 0.65f),
    done = Color(0xFF34D399).copy(alpha = 0.9f),
    time = Color(0xFFA9A9AE).copy(alpha = 0.5f),
)

val LocalZeronColors = staticCompositionLocalOf { ZeronLight }

fun markFile(harness: String?): String = when (harness) {
    "codex" -> "openai"
    "cursor" -> "cursor"
    "devin" -> "devin"
    "grok" -> "grok"
    "hermes" -> "hermes"
    "pi" -> "pi"
    "opencode" -> "opencode"
    "antigravity" -> "antigravity"
    else -> "claude"
}
