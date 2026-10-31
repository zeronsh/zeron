package sh.zeron.android.design

import android.os.Build
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.ColorScheme
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.MaterialExpressiveTheme
import androidx.compose.material3.MotionScheme
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.material3.dynamicLightColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.remember
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.sp

/** Zeron Light / Zeron Dark (crates/theme builtins): cool neutrals, violet accent. */
private val ZeronLight = lightColorScheme(
    primary = Color(0xFF5B43E8),
    onPrimary = Color.White,
    primaryContainer = Color(0xFFE5E0FF),
    onPrimaryContainer = Color(0xFF1E0E6B),
    inversePrimary = Color(0xFF8B7CF6),
    secondary = Color(0xFF5E5A71),
    onSecondary = Color.White,
    secondaryContainer = Color(0xFFE4E1F0),
    onSecondaryContainer = Color(0xFF1C1A29),
    tertiary = Color(0xFF0F766E),
    onTertiary = Color.White,
    tertiaryContainer = Color(0xFFCCF2EC),
    onTertiaryContainer = Color(0xFF00201D),
    background = Color(0xFFF3F3F5),
    onBackground = Color(0xFF27272C),
    surface = Color(0xFFF3F3F5),
    onSurface = Color(0xFF27272C),
    surfaceVariant = Color(0xFFE7E7EB),
    onSurfaceVariant = Color(0xFF62626A),
    surfaceTint = Color(0xFF5B43E8),
    surfaceBright = Color(0xFFFFFFFF),
    surfaceDim = Color(0xFFE2E2E6),
    surfaceContainerLowest = Color(0xFFFFFFFF),
    surfaceContainerLow = Color(0xFFFAFAFB),
    surfaceContainer = Color(0xFFEEEEF1),
    surfaceContainerHigh = Color(0xFFE8E8EC),
    surfaceContainerHighest = Color(0xFFE2E2E7),
    outline = Color(0xFF97979F),
    outlineVariant = Color(0xFFE2E2E6),
    error = Color(0xFFDC2626),
    onError = Color.White,
    errorContainer = Color(0xFFFDE2E2),
    onErrorContainer = Color(0xFF5F1111),
    inverseSurface = Color(0xFF27272C),
    inverseOnSurface = Color(0xFFF3F3F5),
    scrim = Color.Black,
)

private val ZeronDark = darkColorScheme(
    primary = Color(0xFF8B7CF6),
    onPrimary = Color(0xFF1A0F5C),
    primaryContainer = Color(0xFF2F2670),
    onPrimaryContainer = Color(0xFFE5E0FF),
    inversePrimary = Color(0xFF5B43E8),
    secondary = Color(0xFFC8C3DC),
    onSecondary = Color(0xFF302D40),
    secondaryContainer = Color(0xFF26242F),
    onSecondaryContainer = Color(0xFFE4E1F0),
    tertiary = Color(0xFF5EEAD4),
    onTertiary = Color(0xFF003731),
    tertiaryContainer = Color(0xFF0C3A35),
    onTertiaryContainer = Color(0xFFCCF2EC),
    background = Color(0xFF060606),
    onBackground = Color(0xFFE8E8EA),
    surface = Color(0xFF060606),
    onSurface = Color(0xFFE8E8EA),
    surfaceVariant = Color(0xFF1C1C20),
    onSurfaceVariant = Color(0xFFA9A9AE),
    surfaceTint = Color(0xFF8B7CF6),
    surfaceBright = Color(0xFF26262A),
    surfaceDim = Color(0xFF060606),
    surfaceContainerLowest = Color(0xFF000000),
    surfaceContainerLow = Color(0xFF0E0E10),
    surfaceContainer = Color(0xFF131316),
    surfaceContainerHigh = Color(0xFF1A1A1E),
    surfaceContainerHighest = Color(0xFF222227),
    outline = Color(0xFF6B6B72),
    outlineVariant = Color(0xFF1E1E22),
    error = Color(0xFFF87171),
    onError = Color(0xFF450A0A),
    errorContainer = Color(0xFF3B1212),
    onErrorContainer = Color(0xFFFDE2E2),
    inverseSurface = Color(0xFFE8E8EA),
    inverseOnSurface = Color(0xFF111113),
    scrim = Color.Black,
)

/** Geist, from the same asset bytes the transcript measures. */
val Geist: FontFamily by lazy {
    FontFamily(
    Font("Geist.ttf", LocalAssets.manager, FontWeight.Normal),
    Font("Geist-Medium.ttf", LocalAssets.manager, FontWeight.Medium),
    Font("Geist-SemiBold.ttf", LocalAssets.manager, FontWeight.SemiBold),
    Font("Geist-Bold.ttf", LocalAssets.manager, FontWeight.Bold),
    Font("Geist-Italic.ttf", LocalAssets.manager, FontWeight.Normal, FontStyle.Italic),
    )
}

val GeistMono: FontFamily by lazy {
    FontFamily(
    Font("GeistMono.ttf", LocalAssets.manager, FontWeight.Normal),
    Font("GeistMono-Medium.ttf", LocalAssets.manager, FontWeight.Medium),
    Font("GeistMono-SemiBold.ttf", LocalAssets.manager, FontWeight.SemiBold),
    )
}

/** App-wide AssetManager for font families declared at top level. */
object LocalAssets {
    lateinit var manager: android.content.res.AssetManager
}

private fun TextStyle.geist() = copy(fontFamily = Geist)

private fun zeronTypography(): Typography {
    val base = Typography()
    fun TextStyle.w(weight: FontWeight, tracking: Float? = null) =
        geist().copy(fontWeight = weight, letterSpacing = tracking?.sp ?: letterSpacing)
    return Typography(
        displayLarge = base.displayLarge.w(FontWeight.SemiBold, -1.5f),
        displayMedium = base.displayMedium.w(FontWeight.SemiBold, -1.2f),
        displaySmall = base.displaySmall.w(FontWeight.SemiBold, -1f),
        headlineLarge = base.headlineLarge.w(FontWeight.SemiBold, -0.6f),
        headlineMedium = base.headlineMedium.w(FontWeight.SemiBold, -0.4f),
        headlineSmall = base.headlineSmall.w(FontWeight.SemiBold, -0.3f),
        titleLarge = base.titleLarge.w(FontWeight.SemiBold, -0.2f),
        titleMedium = base.titleMedium.w(FontWeight.Medium),
        titleSmall = base.titleSmall.w(FontWeight.Medium),
        bodyLarge = base.bodyLarge.geist().copy(fontSize = 16.5.sp, lineHeight = 24.sp, letterSpacing = 0.sp),
        bodyMedium = base.bodyMedium.geist().copy(letterSpacing = 0.sp),
        bodySmall = base.bodySmall.geist().copy(letterSpacing = 0.sp),
        labelLarge = base.labelLarge.w(FontWeight.Medium, 0f),
        labelMedium = base.labelMedium.w(FontWeight.Medium, 0f),
        labelSmall = base.labelSmall.w(FontWeight.Medium, 0f),
        displayLargeEmphasized = base.displayLarge.w(FontWeight.Bold, -1.5f),
        displayMediumEmphasized = base.displayMedium.w(FontWeight.Bold, -1.2f),
        displaySmallEmphasized = base.displaySmall.w(FontWeight.Bold, -1f),
        headlineLargeEmphasized = base.headlineLarge.w(FontWeight.Bold, -0.6f),
        headlineMediumEmphasized = base.headlineMedium.w(FontWeight.Bold, -0.4f),
        headlineSmallEmphasized = base.headlineSmall.w(FontWeight.Bold, -0.3f),
        titleLargeEmphasized = base.titleLarge.w(FontWeight.Bold, -0.2f),
        titleMediumEmphasized = base.titleMedium.w(FontWeight.SemiBold),
        titleSmallEmphasized = base.titleSmall.w(FontWeight.SemiBold),
        bodyLargeEmphasized = base.bodyLarge.w(FontWeight.Medium, 0f).copy(fontSize = 16.5.sp, lineHeight = 24.sp),
        bodyMediumEmphasized = base.bodyMedium.w(FontWeight.Medium, 0f),
        bodySmallEmphasized = base.bodySmall.w(FontWeight.Medium, 0f),
        labelLargeEmphasized = base.labelLarge.w(FontWeight.SemiBold, 0f),
        labelMediumEmphasized = base.labelMedium.w(FontWeight.SemiBold, 0f),
        labelSmallEmphasized = base.labelSmall.w(FontWeight.SemiBold, 0f),
    )
}

enum class ThemeMode { System, Light, Dark }

data class Appearance(val mode: ThemeMode = ThemeMode.System, val dynamicColor: Boolean = false)

/** Whether the resolved theme is dark (the transcript palette follows it). */
val LocalDarkTheme = staticCompositionLocalOf { false }

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun ZeronTheme(appearance: Appearance = Appearance(), content: @Composable () -> Unit) {
    val dark = when (appearance.mode) {
        ThemeMode.System -> isSystemInDarkTheme()
        ThemeMode.Light -> false
        ThemeMode.Dark -> true
    }
    val context = LocalContext.current
    val scheme: ColorScheme = when {
        appearance.dynamicColor && Build.VERSION.SDK_INT >= Build.VERSION_CODES.S ->
            if (dark) dynamicDarkColorScheme(context) else dynamicLightColorScheme(context)
        dark -> ZeronDark
        else -> ZeronLight
    }
    val typography = remember { zeronTypography() }
    CompositionLocalProvider(LocalDarkTheme provides dark) {
        MaterialExpressiveTheme(
            colorScheme = scheme,
            motionScheme = MotionScheme.expressive(),
            typography = typography,
            content = content,
        )
    }
}
