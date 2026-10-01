package sh.zeron.android.design

import android.content.Context
import android.graphics.Paint
import android.graphics.Typeface
import android.graphics.fonts.Font
import android.graphics.fonts.FontFamily
import android.graphics.fonts.FontStyle
import android.graphics.fonts.SystemFonts
import android.os.Build
import android.util.Log
import java.io.File
import uniffi.zeron_core.FaceRole

/**
 * One font fallback chain for everything Zeron draws: Geist (or Geist Mono)
 * first, then Noto Sans CJK SC from the system image, then the platform's own
 * fallback. The Rust core measures Geist-covered text itself and asks
 * [sh.zeron.android.core.AndroidMeasurer] for the rest; that measurer, the
 * transcript painter and the Compose UI all resolve through these same
 * Typefaces, so wrapping, heights and drawing agree.
 *
 * In monospace faces an East Asian wide character takes exactly two cells
 * ([cellWidth] × 2), in measurement and in drawing.
 */
object FontChain {
    private const val TAG = "FontChain"

    val files: Map<FaceRole, String> = linkedMapOf(
        FaceRole.SANS to "Geist.ttf",
        FaceRole.SANS_MEDIUM to "Geist-Medium.ttf",
        FaceRole.SANS_SEMIBOLD to "Geist-SemiBold.ttf",
        FaceRole.SANS_BOLD to "Geist-Bold.ttf",
        FaceRole.SANS_ITALIC to "Geist-Italic.ttf",
        FaceRole.SANS_MEDIUM_ITALIC to "Geist-MediumItalic.ttf",
        FaceRole.SANS_SEMIBOLD_ITALIC to "Geist-SemiBoldItalic.ttf",
        FaceRole.SANS_BOLD_ITALIC to "Geist-BoldItalic.ttf",
        FaceRole.MONO to "GeistMono.ttf",
        FaceRole.MONO_MEDIUM to "GeistMono-Medium.ttf",
        FaceRole.MONO_SEMIBOLD to "GeistMono-SemiBold.ttf",
        FaceRole.MONO_ITALIC to "GeistMono-Italic.ttf",
    )

    @Volatile private var loaded: Map<FaceRole, Typeface>? = null
    /** Which CJK file the chain found (for diagnostics / the README). */
    @Volatile var cjkSource: String? = null
        private set

    fun faces(context: Context): Map<FaceRole, Typeface> {
        loaded?.let { return it }
        synchronized(this) {
            loaded?.let { return it }
            val app = context.applicationContext
            val built = files.mapValues { (role, name) -> build(app, role, name) }
            loaded = built
            return built
        }
    }

    fun face(context: Context, role: FaceRole): Typeface = faces(context)[role] ?: faces(context).getValue(FaceRole.SANS)

    fun weightOf(role: FaceRole): Int = when (role) {
        FaceRole.SANS, FaceRole.SANS_ITALIC, FaceRole.MONO, FaceRole.MONO_ITALIC -> 400
        FaceRole.SANS_MEDIUM, FaceRole.SANS_MEDIUM_ITALIC, FaceRole.MONO_MEDIUM -> 500
        FaceRole.SANS_SEMIBOLD, FaceRole.SANS_SEMIBOLD_ITALIC, FaceRole.MONO_SEMIBOLD -> 600
        FaceRole.SANS_BOLD, FaceRole.SANS_BOLD_ITALIC -> 700
    }

    fun isItalic(role: FaceRole): Boolean = when (role) {
        FaceRole.SANS_ITALIC, FaceRole.SANS_MEDIUM_ITALIC, FaceRole.SANS_SEMIBOLD_ITALIC,
        FaceRole.SANS_BOLD_ITALIC, FaceRole.MONO_ITALIC -> true
        else -> false
    }

    fun isMono(role: FaceRole): Boolean = when (role) {
        FaceRole.MONO, FaceRole.MONO_MEDIUM, FaceRole.MONO_SEMIBOLD, FaceRole.MONO_ITALIC -> true
        else -> false
    }

    private fun build(context: Context, role: FaceRole, name: String): Typeface {
        val plain = runCatching { Typeface.createFromAsset(context.assets, name) }.getOrElse { Typeface.DEFAULT }
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return plain
        return try {
            val weight = weightOf(role)
            val slant = if (isItalic(role)) FontStyle.FONT_SLANT_ITALIC else FontStyle.FONT_SLANT_UPRIGHT
            val primary = Font.Builder(context.assets, name).setWeight(weight).setSlant(slant).build()
            val builder = Typeface.CustomFallbackBuilder(FontFamily.Builder(primary).build())
            cjkFamily(weight)?.let { builder.addCustomFallback(it) }
            builder.setSystemFallback("sans-serif")
            builder.setStyle(FontStyle(weight, slant))
            builder.build()
        } catch (t: Throwable) {
            Log.w(TAG, "custom fallback chain failed for $name", t)
            plain
        }
    }

    private class CjkFile(val file: File, val ttcIndex: Int, val variable: Boolean)

    private val cjk: CjkFile? by lazy { findCjk() }

    /** Noto Sans CJK, Simplified Chinese face, from the system font set. */
    private fun findCjk(): CjkFile? {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return null
        val fonts = runCatching { SystemFonts.getAvailableFonts() }.getOrNull().orEmpty()
        val candidates = fonts.filter { f ->
            val path = f.file?.name ?: return@filter false
            path.contains("CJK", ignoreCase = true) && !path.contains("Serif", ignoreCase = true)
        }
        val hans = candidates.firstOrNull { f ->
            val locales = f.localeList
            (0 until locales.size()).any { i -> locales[i].toLanguageTag().startsWith("zh-Hans") || locales[i].toLanguageTag() == "zh-CN" }
        } ?: candidates.firstOrNull { it.file?.name?.startsWith("NotoSansCJK") == true && it.ttcIndex == 2 }
            ?: candidates.firstOrNull()
        val picked = hans ?: run {
            val fallback = File("/system/fonts/NotoSansCJK-Regular.ttc")
            return if (fallback.exists()) CjkFile(fallback, 2, true).also { cjkSource = "${fallback.name}#2" } else null
        }
        val file = picked.file ?: return null
        val variable = picked.axes?.any { it.tag == "wght" } == true || file.name.contains("VF", ignoreCase = true) || file.name == "NotoSansCJK-Regular.ttc"
        cjkSource = "${file.name}#${picked.ttcIndex}"
        Log.i(TAG, "CJK fallback: $cjkSource variable=$variable")
        return CjkFile(file, picked.ttcIndex, variable)
    }

    private val cjkFamilies = HashMap<Int, FontFamily?>()

    private fun cjkFamily(weight: Int): FontFamily? = synchronized(cjkFamilies) {
        cjkFamilies.getOrPut(weight) {
            val source = cjk ?: return@getOrPut null
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return@getOrPut null
            runCatching {
                val b = Font.Builder(source.file).setTtcIndex(source.ttcIndex).setWeight(weight).setSlant(FontStyle.FONT_SLANT_UPRIGHT)
                if (source.variable) b.setFontVariationSettings("'wght' $weight")
                FontFamily.Builder(b.build()).build()
            }.getOrNull()
        }
    }

    /** East Asian Wide / Fullwidth code points (UAX #11), plus wide emoji. */
    fun isWide(cp: Int): Boolean = when {
        cp < 0x1100 -> false
        cp <= 0x115F -> true
        cp in 0x2E80..0x303E -> true
        cp in 0x3041..0x33FF -> true
        cp in 0x3400..0x4DBF -> true
        cp in 0x4E00..0x9FFF -> true
        cp in 0xA000..0xA4CF -> true
        cp in 0xA960..0xA97F -> true
        cp in 0xAC00..0xD7A3 -> true
        cp in 0xF900..0xFAFF -> true
        cp in 0xFE10..0xFE19 -> true
        cp in 0xFE30..0xFE6F -> true
        cp in 0xFF00..0xFF60 -> true
        cp in 0xFFE0..0xFFE6 -> true
        cp in 0x1F300..0x1F64F -> true
        cp in 0x1F900..0x1F9FF -> true
        cp in 0x20000..0x3FFFD -> true
        else -> false
    }

    fun hasWide(text: CharSequence, start: Int = 0, end: Int = text.length): Boolean {
        var i = start
        while (i < end) {
            val cp = Character.codePointAt(text, i)
            if (isWide(cp)) return true
            i += Character.charCount(cp)
        }
        return false
    }

    /** One monospace cell at the paint's current size (Geist Mono's advance). */
    fun cellWidth(paint: Paint): Float = paint.measureText("0")
}
