package sh.zeron.android.design

import androidx.compose.ui.graphics.Color
import kotlin.math.max
import kotlin.math.min
import kotlin.math.pow
import kotlin.math.roundToInt

/** One built-in theme variant's seeds (see ThemeSeeds.kt, generated from the desktop's builtins.rs). */
internal class ThemeSeed(
    val id: String,
    val family: String,
    val name: String,
    val dark: Boolean,
    val background: Int,
    val shell: Int,
    val raised: Int,
    val card: Int,
    val text: Int,
    val muted: Int,
    val faint: Int,
    val accent: Int,
    val danger: Int,
    val warning: Int,
    val success: Int,
    /** comment, keyword, string, number, type, function, property, variable, punctuation, tag, attribute, invalid. */
    val syntax: IntArray,
)

/** Theme syntax colors (the desktop's `syntax` map, by the roles the transcript uses). */
data class ThemeSyntax(
    val comment: Color,
    val keyword: Color,
    val string: Color,
    val number: Color,
    val type: Color,
    val function: Color,
    val property: Color,
    val variable: Color,
    val punctuation: Color,
    val tag: Color,
    val attribute: Color,
)

/**
 * The desktop's accent options (crates/theme AccentPreset): "Theme default"
 * plus seven presets, each with a dark and a light color.
 */
enum class AccentChoice(val key: String, val label: String, private val darkHex: Int, private val lightHex: Int) {
    THEME("theme", "Theme default", 0, 0),
    ZERON("zeron", "Zeron", 0x8B7CF6, 0x5B43E8),
    ORANGE("orange", "Orange", 0xFB923C, 0xC2410C),
    AMBER("amber", "Amber", 0xFBBF24, 0xA16207),
    GREEN("green", "Green", 0x4ADE80, 0x15803D),
    CYAN("cyan", "Cyan", 0x22D3EE, 0x0E7490),
    BLUE("blue", "Blue", 0x60A5FA, 0x2563EB),
    PINK("pink", "Pink", 0xF472B6, 0xBE185D),
    ;

    /** The preset's color for [dark] appearance; null for THEME (the theme's own accent). */
    fun hex(dark: Boolean): Int? = if (this == THEME) null else if (dark) darkHex else lightHex

    companion object {
        fun of(key: String?): AccentChoice = entries.firstOrNull { it.key == key } ?: THEME
    }
}

/**
 * Theme + accent resolution, ported from zeronsh/zeron crates/theme
 * (lib.rs Color math and AccentRoles::derive, builtins.rs variant()).
 * Zeron Light / Zeron Dark keep the phone's hand-tuned palettes (the iOS
 * Palette values); every other theme is derived from its seeds the way the
 * desktop derives its surfaces. Semantic colors (success / warning / danger)
 * always come from the theme, never from the accent.
 */
object ZeronThemes {
    const val DEFAULT_LIGHT = "zeron-light"
    const val DEFAULT_DARK = "zeron-dark"

    internal val all: List<ThemeSeed> get() = ThemeSeeds

    /** Variants for one appearance, in the desktop's order ("Light theme" / "Dark theme" pickers). */
    internal fun variants(dark: Boolean): List<ThemeSeed> = ThemeSeeds.filter { it.dark == dark }

    internal fun seed(id: String?, dark: Boolean): ThemeSeed =
        ThemeSeeds.firstOrNull { it.id == id && it.dark == dark } ?: ThemeSeeds.first { it.id == if (dark) DEFAULT_DARK else DEFAULT_LIGHT }

    fun name(id: String?, dark: Boolean): String = seed(id, dark).name

    /** The palette for the current appearance: its selected theme, then the accent. */
    fun colors(dark: Boolean, lightId: String?, darkId: String?, accent: AccentChoice): ZeronColors {
        val seed = seed(if (dark) darkId else lightId, dark)
        val base = if (seed.id == DEFAULT_LIGHT) ZeronLight else if (seed.id == DEFAULT_DARK) ZeronDark else derive(seed)
        val preset = accent.hex(dark) ?: return base
        return withAccent(base, AccentRoles.derive(preset, dark, seed.background))
    }

    /** Swatch color for an accent option in the picker (THEME = the theme's own accent). */
    fun swatch(choice: AccentChoice, dark: Boolean, themeId: String?): Color {
        val seed = seed(themeId, dark)
        return Color(0xFF000000.toInt() or AccentRoles.derive(choice.hex(dark) ?: seed.accent, dark, seed.background).primary)
    }

    /** Preview dots for a theme option: page, card, accent, text. */
    internal fun preview(seed: ThemeSeed): List<Color> {
        val page = if (seed.dark) seed.background else seed.shell
        return listOf(page, seed.card, AccentRoles.derive(seed.accent, seed.dark, seed.background).primary, seed.text).map { argb(it) }
    }

    private fun withAccent(base: ZeronColors, roles: AccentRoles): ZeronColors {
        val a = argb(roles.primary)
        return base.copy(
            accent = a,
            accentSoft = a.copy(alpha = 0.12f),
            working = a.copy(alpha = 0.55f),
            input = a.copy(alpha = 0.6f),
            glyph = roles.glyph.map { argb(it) },
            undoTint = argb(Rgb.ensureContrast(roles.primary, Rgb.of(base.text), 3.0f)),
        )
    }

    internal fun derive(seed: ThemeSeed): ZeronColors {
        val dark = seed.dark
        val page = if (dark) seed.background else seed.shell
        var elevated = seed.card
        if (Rgb.contrast(elevated, page) < 1.06f) elevated = Rgb.mix(page, seed.text, if (dark) 0.06f else 0.035f)
        val text = argb(seed.text)
        val secondary = argb(Rgb.ensureContrast(seed.muted, seed.background, 4.5f))
        val tertiary = argb(seed.faint)
        val hairline = argb(Rgb.blend(if (dark) 0xFFFFFF else 0x000000, if (dark) 0.10f else 0.12f, page))
        val roles = AccentRoles.derive(seed.accent, dark, seed.background)
        val accent = argb(roles.primary)
        val danger = argb(seed.danger)
        val success = argb(seed.success)
        val s = seed.syntax.map { argb(it) }
        return ZeronColors(
            dark = dark,
            background = argb(page),
            elevated = argb(elevated),
            text = text,
            secondary = secondary,
            tertiary = tertiary,
            hairline = hairline,
            accent = accent,
            controlFill = text.copy(alpha = 0.075f),
            accentSoft = accent.copy(alpha = 0.12f),
            danger = danger,
            success = success,
            warning = argb(seed.warning),
            userBubble = argb(if (dark) Rgb.mix(elevated, seed.text, 0.04f) else elevated),
            codeBackground = argb(if (dark) Rgb.mix(page, seed.text, 0.025f) else Rgb.mix(page, 0xFFFFFF, 0.5f)),
            codeBorder = hairline,
            chip = argb(Rgb.mix(page, seed.text, if (dark) 0.08f else 0.06f)),
            subline = secondary.copy(alpha = 0.5f),
            rowActive = if (dark) Color.White.copy(alpha = 0.06f) else Color.White.copy(alpha = 0.9f),
            projectTones = if (dark) ZeronDark.projectTones else ZeronLight.projectTones,
            working = accent.copy(alpha = 0.55f),
            input = accent.copy(alpha = 0.6f),
            failed = danger.copy(alpha = 0.65f),
            done = success.copy(alpha = 0.9f),
            time = secondary.copy(alpha = 0.5f),
            syntax = ThemeSyntax(
                comment = s[0], keyword = s[1], string = s[2], number = s[3], type = s[4], function = s[5],
                property = s[6], variable = s[7], punctuation = s[8], tag = s[9], attribute = s[10],
            ),
            glyph = roles.glyph.map { argb(it) },
            undoTint = argb(Rgb.ensureContrast(roles.primary, seed.text, 3.0f)),
            themeId = seed.id,
            shell = argb(if (dark) seed.shell else page),
            sheet = argb(if (dark) Rgb.mix(elevated, seed.text, 0.05f) else elevated),
        )
    }

    private fun argb(rgb: Int) = Color(0xFF000000.toInt() or (rgb and 0xFFFFFF))
}

/** The desktop's AccentRoles (the parts the phone paints with). */
internal class AccentRoles(val primary: Int, val glyph: List<Int>) {
    companion object {
        fun derive(seed: Int, dark: Boolean, background: Int): AccentRoles {
            val primary = Rgb.ensureContrast(seed, background, 3.0f)
            val light = Rgb.mix(primary, if (dark) 0xFFFFFF else background, if (dark) 0.28f else 0.18f)
            val deep = Rgb.mix(primary, 0x000000, if (dark) 0.18f else 0.26f)
            return AccentRoles(primary, listOf(light, primary, deep))
        }
    }
}

/** crates/theme Color math on opaque 0xRRGGBB ints (same rounding as the Rust). */
internal object Rgb {
    fun of(c: Color): Int = ((c.red * 255).roundToInt() shl 16) or ((c.green * 255).roundToInt() shl 8) or (c.blue * 255).roundToInt()

    private fun r(c: Int) = (c shr 16) and 0xFF
    private fun g(c: Int) = (c shr 8) and 0xFF
    private fun b(c: Int) = c and 0xFF

    fun mix(a: Int, b: Int, amount: Float): Int {
        val t = amount.coerceIn(0f, 1f)
        fun m(x: Int, y: Int) = (x + (y - x) * t).roundToInt()
        return (m(r(a), r(b)) shl 16) or (m(g(a), g(b)) shl 8) or m(b(a), b(b))
    }

    /** [front] at [alpha] over [back]. */
    fun blend(front: Int, alpha: Float, back: Int): Int {
        val a = ((alpha.coerceIn(0f, 1f) * 255f).roundToInt()) / 255f
        fun m(x: Int, y: Int) = (x * a + y * (1 - a)).roundToInt()
        return (m(r(front), r(back)) shl 16) or (m(g(front), g(back)) shl 8) or m(b(front), b(back))
    }

    private fun luminance(c: Int): Float {
        fun lin(ch: Int): Float {
            val v = ch / 255f
            return if (v <= 0.04045f) v / 12.92f else ((v + 0.055f) / 1.055f).pow(2.4f)
        }
        return 0.2126f * lin(r(c)) + 0.7152f * lin(g(c)) + 0.0722f * lin(b(c))
    }

    fun contrast(a: Int, b: Int): Float {
        val la = luminance(a)
        val lb = luminance(b)
        return (max(la, lb) + 0.05f) / (min(la, lb) + 0.05f)
    }

    /** Move toward black or white (whichever contrasts more) until [minimum] is met. */
    fun ensureContrast(c: Int, background: Int, minimum: Float): Int {
        if (contrast(c, background) >= minimum) return c
        val target = if (contrast(0x000000, background) >= contrast(0xFFFFFF, background)) 0x000000 else 0xFFFFFF
        for (step in 1..20) {
            val candidate = mix(c, target, step / 20f)
            if (contrast(candidate, background) >= minimum) return candidate
        }
        return target
    }
}
