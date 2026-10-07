package sh.zeron.android.design

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.compositeOver
import androidx.compose.ui.text.TextStyle

/**
 * Material components the app still uses (AlertDialog, TextButton, text
 * fields, pull-to-refresh) read MaterialTheme. Without one they fall back to
 * the stock light-purple scheme even in dark mode, so derive it from the
 * Zeron palette: dialogs sit on the elevated surface, buttons use the accent.
 */
@Composable
fun ZeronMaterialTheme(colors: ZeronColors, content: @Composable () -> Unit) {
    val scheme = remember(colors) {
        val surface = colors.elevated
        val raised = if (colors.dark) Color(0xFF1C1C1F) else Color.White
        val base = if (colors.dark) darkColorScheme() else lightColorScheme()
        base.copy(
            primary = colors.accent,
            onPrimary = Color.White,
            primaryContainer = colors.accentSoft.compositeOver(surface),
            onPrimaryContainer = colors.text,
            secondary = colors.secondary,
            onSecondary = colors.background,
            background = colors.background,
            onBackground = colors.text,
            surface = surface,
            onSurface = colors.text,
            surfaceVariant = raised,
            onSurfaceVariant = colors.secondary,
            surfaceTint = Color.Transparent,
            surfaceContainerLowest = surface,
            surfaceContainerLow = surface,
            surfaceContainer = raised,
            surfaceContainerHigh = raised,
            surfaceContainerHighest = raised,
            inverseSurface = colors.text,
            inverseOnSurface = colors.background,
            outline = colors.tertiary,
            outlineVariant = colors.hairline,
            error = colors.danger,
            onError = Color.White,
            scrim = Color.Black,
        )
    }
    val typography = remember {
        val t = Typography()
        fun TextStyle.sans() = copy(fontFamily = ZeronType.Sans)
        t.copy(
            headlineSmall = t.headlineSmall.sans(),
            titleLarge = t.titleLarge.sans(),
            titleMedium = t.titleMedium.sans(),
            bodyLarge = t.bodyLarge.sans(),
            bodyMedium = t.bodyMedium.sans(),
            labelLarge = t.labelLarge.sans(),
        )
    }
    MaterialTheme(colorScheme = scheme, typography = typography, content = content)
}
